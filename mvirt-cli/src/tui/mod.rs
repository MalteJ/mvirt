use std::io;
use std::time::Duration;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::*;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use crate::net_proto::net_service_client::NetServiceClient;
use crate::proto::vm_service_client::VmServiceClient;
use crate::zfs_proto::zfs_service_client::ZfsServiceClient;
use mvirt_log::LogServiceClient;

mod app;
pub mod modals;
pub mod types;
pub mod views;
pub mod widgets;
mod worker;

use app::App;
use types::{Action, NetworkFocus, StorageFocus, View};

fn draw(frame: &mut Frame, app: &mut App) {
    // Draw base view based on active view
    match app.active_view {
        View::Vm => {
            views::vms::draw(
                frame,
                &app.vms,
                &mut app.table_state,
                app.system_info.as_ref(),
                app.status_message.as_deref(),
                app.confirm_delete.as_deref(),
                app.confirm_kill.as_deref(),
                app.last_refresh,
            );
        }
        View::Storage => {
            views::storage::draw(
                frame,
                &app.storage,
                &mut app.volume_table_state,
                &mut app.template_table_state,
                app.storage_focus,
                app.status_message.as_deref(),
                app.confirm_delete_volume.as_deref(),
                app.confirm_delete_template.as_deref(),
                app.last_refresh,
            );
        }
        View::Logs => {
            views::logs::draw(
                frame,
                &app.logs,
                &mut app.logs_table_state,
                app.status_message.as_deref(),
                app.last_refresh,
            );
        }
        View::Network => {
            views::network::draw(
                frame,
                &app.network,
                &mut app.networks_table_state,
                &mut app.nics_table_state,
                app.network_focus,
                app.status_message.as_deref(),
                app.confirm_delete_network.as_deref(),
                app.confirm_delete_nic.as_deref(),
                app.last_refresh,
            );
        }
    }

    // Detail View overlay (VM only)
    if let Some(ref vm_id) = app.detail_view
        && let Some(vm) = app.get_vm_by_id(vm_id)
    {
        modals::vm_detail::draw(frame, vm);
    }

    // Create VM Modal overlay
    if let Some(modal) = &app.create_modal {
        modals::vm_create::draw(frame, modal);
    }

    // File Picker overlay (on top of modal)
    if let Some(picker) = &app.file_picker {
        widgets::file_picker::draw(frame, picker);
    }

    // SSH Keys Modal overlay (on top of create modal)
    if let Some(modal) = &app.ssh_keys_modal {
        modals::ssh_keys::draw(frame, modal);
    }

    // Storage modals
    if let Some(modal) = &app.volume_create_modal {
        modals::volume_create::draw(frame, modal);
    }
    if let Some(modal) = &app.volume_import_modal {
        modals::volume_import::draw(frame, modal);
    }
    if let Some(modal) = &app.volume_resize_modal {
        modals::volume_resize::draw(frame, modal);
    }
    if let Some(modal) = &app.volume_snapshot_modal {
        modals::volume_snapshot::draw(frame, modal);
    }
    if let Some(modal) = &app.volume_template_modal {
        modals::volume_template::draw(frame, modal);
    }
    if let Some(modal) = &app.volume_clone_modal {
        modals::volume_clone::draw(frame, modal);
    }

    // Network modals
    if let Some(modal) = &app.network_create_modal {
        modals::network_create::draw(frame, modal);
    }
    if let Some(modal) = &app.nic_create_modal {
        modals::nic_create::draw(frame, modal);
    }

    // Log detail modal
    if let Some(idx) = app.log_detail_index
        && let Some(entry) = app.logs.get(idx)
    {
        modals::log_detail::draw(frame, entry);
    }

    // Console takes over the whole screen
    if let Some(ref mut session) = app.console_session {
        widgets::console::draw(frame, session);
    }
}

pub async fn run(
    vm_client: Option<VmServiceClient<Channel>>,
    zfs_client: Option<ZfsServiceClient<Channel>>,
    log_client: Option<LogServiceClient<Channel>>,
    net_client: Option<NetServiceClient<Channel>>,
) -> io::Result<()> {
    let (action_tx, action_rx) = mpsc::unbounded_channel();
    let (result_tx, result_rx) = mpsc::unbounded_channel();

    let vm_available = vm_client.is_some();
    let zfs_available = zfs_client.is_some();
    let log_available = log_client.is_some();
    let net_available = net_client.is_some();
    tokio::spawn(worker::action_worker(
        vm_client, zfs_client, log_client, net_client, action_rx, result_tx,
    ));

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Show splash screen for 1 second
    terminal.draw(views::splash::draw)?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut app = App::new(
        action_tx,
        result_rx,
        vm_available,
        zfs_available,
        log_available,
        net_available,
    );
    if !vm_available {
        app.status_message = Some("Not connected to mvirt-vmm".to_string());
    }
    if vm_available {
        app.refresh();
    }

    let mut last_refresh = std::time::Instant::now();

    loop {
        // Check for results from background worker
        while let Ok(result) = app.result_rx.try_recv() {
            app.handle_result(result);
            last_refresh = std::time::Instant::now();
        }

        // Auto-refresh every 2 seconds
        if last_refresh.elapsed() >= Duration::from_secs(2) && !app.busy {
            match app.active_view {
                View::Vm => {
                    if app.vm_available {
                        app.send_action(Action::Refresh);
                    }
                }
                View::Storage => {
                    if app.zfs_available {
                        app.send_action(Action::RefreshStorage);
                    }
                }
                View::Logs => {
                    if app.log_available {
                        app.send_action(Action::RefreshLogs { limit: 100 });
                    }
                }
                View::Network => {
                    if app.net_available {
                        app.send_action(Action::RefreshNetworks);
                    }
                }
            }
            last_refresh = std::time::Instant::now();
        }

        terminal.draw(|frame| draw(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            // Console mode takes highest priority
            if let Some(ref mut session) = app.console_session {
                if session.handle_key(key.code, key.modifiers) {
                    app.close_console();
                }
                continue;
            }

            // Handle SSH keys modal
            if app.ssh_keys_modal.is_some() {
                handle_ssh_keys_modal_input(&mut app, key.code);
            } else if app.file_picker.is_some() {
                handle_file_picker_input(&mut app, key.code);
            } else if app.detail_view.is_some() {
                handle_detail_view_input(&mut app, key.code);
            } else if app.create_modal.is_some() {
                handle_create_modal_input(&mut app, key.code);
            } else if app.confirm_kill.is_some() {
                handle_confirm_kill_input(&mut app, key.code);
            } else if app.confirm_delete.is_some() {
                handle_confirm_delete_input(&mut app, key.code);
            } else if app.confirm_delete_volume.is_some() {
                handle_storage_confirm_delete_volume(&mut app, key.code);
            } else if app.confirm_delete_template.is_some() {
                handle_storage_confirm_delete_template(&mut app, key.code);
            } else if app.volume_create_modal.is_some() {
                handle_volume_create_modal_input(&mut app, key.code);
            } else if app.volume_import_modal.is_some() {
                handle_volume_import_modal_input(&mut app, key.code);
            } else if app.volume_resize_modal.is_some() {
                handle_volume_resize_modal_input(&mut app, key.code);
            } else if app.volume_snapshot_modal.is_some() {
                handle_volume_snapshot_modal_input(&mut app, key.code);
            } else if app.volume_template_modal.is_some() {
                handle_volume_template_modal_input(&mut app, key.code);
            } else if app.volume_clone_modal.is_some() {
                handle_volume_clone_modal_input(&mut app, key.code);
            } else if app.network_create_modal.is_some() {
                handle_network_create_modal_input(&mut app, key.code);
            } else if app.nic_create_modal.is_some() {
                handle_nic_create_modal_input(&mut app, key.code);
            } else if app.confirm_delete_network.is_some() {
                handle_confirm_delete_network(&mut app, key.code);
            } else if app.confirm_delete_nic.is_some() {
                handle_confirm_delete_nic(&mut app, key.code);
            } else if app.log_detail_index.is_some() {
                handle_log_detail_input(&mut app, key.code);
            } else {
                handle_normal_input(&mut app, key.code);
            }
        }

        if app.should_quit {
            break;
        }
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    Ok(())
}

fn handle_ssh_keys_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_ssh_keys_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.ssh_keys_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.ssh_keys_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.ssh_keys_modal {
                if modal.is_add_button() {
                    app.confirm_ssh_keys();
                } else if modal.is_cancel_button() {
                    app.close_ssh_keys_modal();
                } else if let Some(modal) = &mut app.ssh_keys_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(modal) = &mut app.ssh_keys_modal
                && modal.is_source_field()
            {
                modal.toggle_source();
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.ssh_keys_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.ssh_keys_modal
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

fn handle_file_picker_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_file_picker(),
        KeyCode::Down => {
            if let Some(picker) = &mut app.file_picker {
                picker.select_next();
            }
        }
        KeyCode::Up => {
            if let Some(picker) = &mut app.file_picker {
                picker.select_prev();
            }
        }
        KeyCode::Enter => app.select_file(),
        _ => {}
    }
}

fn handle_detail_view_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => app.close_detail_view(),
        _ => {}
    }
}

fn handle_create_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_create_modal(),
        KeyCode::Tab => {
            if let Some(modal) = &mut app.create_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab => {
            if let Some(modal) = &mut app.create_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Down => {
            if let Some(modal) = &mut app.create_modal {
                if modal.is_disk_field() {
                    // Navigate disk list
                    modal.disk_select_next();
                } else {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Up => {
            if let Some(modal) = &mut app.create_modal {
                if modal.is_disk_field() {
                    // Navigate disk list
                    modal.disk_select_prev();
                } else {
                    modal.focus_prev();
                }
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.create_modal {
                if modal.is_submit_field() {
                    app.submit_create();
                } else if modal.is_user_data_mode_field() {
                    app.handle_user_data_mode_action();
                } else if let Some(modal) = &mut app.create_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(modal) = &mut app.create_modal {
                if modal.is_disk_field() {
                    modal.toggle_disk_source_type();
                } else if modal.is_nested_virt_field() {
                    modal.toggle_nested_virt();
                } else if modal.is_user_data_mode_field() {
                    modal.cycle_user_data_mode();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.create_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.create_modal {
                if modal.is_numeric_field() {
                    if c.is_ascii_digit()
                        && let Some(input) = modal.current_input()
                    {
                        input.push(c);
                    }
                } else if modal.is_name_field() {
                    if modals::vm_create::CreateModal::is_valid_name_char(c)
                        && let Some(input) = modal.current_input()
                    {
                        input.push(c);
                    }
                } else if let Some(input) = modal.current_input() {
                    input.push(c);
                }
            }
        }
        _ => {}
    }
}

fn handle_confirm_kill_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_kill(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_kill(),
        _ => {}
    }
}

fn handle_confirm_delete_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete(),
        _ => {}
    }
}

fn handle_normal_input(app: &mut App, key_code: KeyCode) {
    // Global keys (all views)
    match key_code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return;
        }
        KeyCode::Tab => {
            app.toggle_view();
            return;
        }
        KeyCode::Char('1') => {
            app.set_view(View::Vm);
            return;
        }
        KeyCode::Char('2') => {
            app.set_view(View::Storage);
            return;
        }
        KeyCode::Char('3') => {
            app.set_view(View::Logs);
            return;
        }
        KeyCode::Char('4') => {
            app.set_view(View::Network);
            return;
        }
        _ => {}
    }

    // View-specific keys
    match app.active_view {
        View::Vm => handle_vm_input(app, key_code),
        View::Storage => handle_storage_input(app, key_code),
        View::Logs => handle_logs_input(app, key_code),
        View::Network => handle_network_input(app, key_code),
    }
}

fn handle_vm_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Down => app.next(),
        KeyCode::Up => app.previous(),
        KeyCode::Enter => app.open_detail_view(),
        KeyCode::Char('n') => app.open_create_modal(),
        KeyCode::Char('c') => app.open_console(),
        KeyCode::Char('s') => app.start_selected(),
        KeyCode::Char('S') => app.stop_selected(),
        KeyCode::Char('k') => app.kill_selected(),
        KeyCode::Char('d') => app.delete_selected(),
        _ => {}
    }
}

fn handle_storage_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Down => app.storage_next(),
        KeyCode::Up => app.storage_previous(),
        KeyCode::BackTab => app.toggle_storage_focus(),
        KeyCode::Char('d') => match app.storage_focus {
            StorageFocus::Volumes => app.delete_selected_volume(),
            StorageFocus::Templates => app.delete_selected_template(),
        },
        KeyCode::Char('n') => app.open_volume_create_modal(),
        KeyCode::Char('i') => app.open_volume_import_modal(),
        KeyCode::Char('r') => {
            if app.storage_focus == StorageFocus::Volumes {
                app.open_volume_resize_modal();
            }
        }
        KeyCode::Char('s') => {
            if app.storage_focus == StorageFocus::Volumes {
                app.open_volume_snapshot_modal();
            }
        }
        KeyCode::Char('t') => {
            if app.storage_focus == StorageFocus::Volumes {
                app.open_volume_template_modal();
            }
        }
        KeyCode::Char('c') => {
            if app.storage_focus == StorageFocus::Templates {
                app.open_volume_clone_modal();
            }
        }
        _ => {}
    }
}

fn handle_logs_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Down => app.logs_next(),
        KeyCode::Up => app.logs_previous(),
        KeyCode::Enter => app.open_log_detail(),
        KeyCode::Char('r') => app.refresh_logs(),
        _ => {}
    }
}

fn handle_log_detail_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc | KeyCode::Enter => app.close_log_detail(),
        _ => {}
    }
}

fn handle_storage_confirm_delete_volume(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete_volume(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete_volume(),
        _ => {}
    }
}

fn handle_storage_confirm_delete_template(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete_template(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete_template(),
        _ => {}
    }
}

fn handle_volume_create_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_create_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_create_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_create_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_create_modal {
                if modal.is_submit_field() {
                    app.submit_volume_create();
                } else if let Some(modal) = &mut app.volume_create_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(modal) = &mut app.volume_create_modal
                && modal.is_unit_field()
            {
                modal.toggle_unit();
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_create_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_create_modal {
                if modal.is_size_field()
                    && c.is_ascii_digit()
                    && let Some(input) = modal.current_input()
                {
                    input.push(c);
                } else if modal.is_name_field()
                    && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && let Some(input) = modal.current_input()
                {
                    input.push(c);
                }
            }
        }
        _ => {}
    }
}

fn handle_volume_import_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_import_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_import_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_import_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_import_modal {
                if modal.is_submit_field() {
                    app.submit_volume_import();
                } else if let Some(modal) = &mut app.volume_import_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_import_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_import_modal {
                if modal.is_size_field()
                    && c.is_ascii_digit()
                    && let Some(input) = modal.current_input()
                {
                    input.push(c);
                } else if modal.is_name_field()
                    && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && let Some(input) = modal.current_input()
                {
                    input.push(c);
                } else if let Some(input) = modal.current_input() {
                    input.push(c);
                }
            }
        }
        _ => {}
    }
}

fn handle_volume_resize_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_resize_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_resize_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_resize_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_resize_modal {
                if modal.is_submit_field() {
                    app.submit_volume_resize();
                } else if let Some(modal) = &mut app.volume_resize_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(modal) = &mut app.volume_resize_modal
                && modal.is_unit_field()
            {
                modal.toggle_unit();
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_resize_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_resize_modal
                && modal.is_size_field()
                && c.is_ascii_digit()
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

fn handle_volume_snapshot_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_snapshot_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_snapshot_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_snapshot_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_snapshot_modal {
                if modal.is_submit_field() {
                    app.submit_volume_snapshot();
                } else if let Some(modal) = &mut app.volume_snapshot_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_snapshot_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_snapshot_modal
                && modal.is_name_field()
                && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

fn handle_volume_template_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_template_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_template_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_template_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_template_modal {
                if modal.is_submit_field() {
                    app.submit_volume_template();
                } else if let Some(modal) = &mut app.volume_template_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_template_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_template_modal
                && modal.is_name_field()
                && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

fn handle_volume_clone_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_volume_clone_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.volume_clone_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.volume_clone_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.volume_clone_modal {
                if modal.is_submit_field() {
                    app.submit_volume_clone();
                } else if let Some(modal) = &mut app.volume_clone_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.volume_clone_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.volume_clone_modal
                && modal.is_name_field()
                && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

// === Network Input Handlers ===

fn handle_network_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Down => app.network_next(),
        KeyCode::Up => app.network_previous(),
        KeyCode::Tab | KeyCode::BackTab => app.toggle_network_focus(),
        KeyCode::Enter => {
            if app.network_focus == NetworkFocus::Networks {
                app.select_network();
            }
        }
        KeyCode::Char('n') => {
            if app.network_focus == NetworkFocus::Networks {
                app.open_network_create_modal();
            }
        }
        KeyCode::Char('c') => {
            if app.network_focus == NetworkFocus::Nics {
                app.open_nic_create_modal();
            }
        }
        KeyCode::Char('d') => match app.network_focus {
            NetworkFocus::Networks => app.delete_selected_network(),
            NetworkFocus::Nics => app.delete_selected_nic(),
        },
        KeyCode::Char('r') => app.refresh_networks(),
        _ => {}
    }
}

fn handle_network_create_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_network_create_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.network_create_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.network_create_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.network_create_modal {
                if modal.is_submit_field() {
                    app.submit_network_create();
                } else if let Some(modal) = &mut app.network_create_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.network_create_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.network_create_modal {
                if modal.is_name_field()
                    && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    && let Some(input) = modal.current_input()
                {
                    input.push(c);
                } else if let Some(input) = modal.current_input() {
                    // IPv4/IPv6 fields accept more characters
                    input.push(c);
                }
            }
        }
        _ => {}
    }
}

fn handle_nic_create_modal_input(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Esc => app.close_nic_create_modal(),
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.nic_create_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.nic_create_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.nic_create_modal {
                if modal.is_submit_field() {
                    app.submit_nic_create();
                } else if let Some(modal) = &mut app.nic_create_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(modal) = &mut app.nic_create_modal
                && let Some(input) = modal.current_input()
            {
                input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(modal) = &mut app.nic_create_modal
                && modal.is_name_field()
                && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
                && let Some(input) = modal.current_input()
            {
                input.push(c);
            }
        }
        _ => {}
    }
}

fn handle_confirm_delete_network(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete_network(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete_network(),
        _ => {}
    }
}

fn handle_confirm_delete_nic(app: &mut App, key_code: KeyCode) {
    match key_code {
        KeyCode::Char('y') | KeyCode::Char('Y') => app.confirm_delete_nic(),
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.cancel_delete_nic(),
        _ => {}
    }
}
