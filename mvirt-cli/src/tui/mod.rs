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

use crate::proto::vm_service_client::VmServiceClient;

mod app;
pub mod modals;
pub mod types;
pub mod views;
pub mod widgets;
mod worker;

use app::App;
use types::Action;

fn draw(frame: &mut Frame, app: &mut App) {
    // Draw base VM list view
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

    // Detail View overlay
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

    // Console takes over the whole screen
    if let Some(ref mut session) = app.console_session {
        widgets::console::draw(frame, session);
    }
}

pub async fn run(client: VmServiceClient<Channel>) -> io::Result<()> {
    let (action_tx, action_rx) = mpsc::unbounded_channel();
    let (result_tx, result_rx) = mpsc::unbounded_channel();

    tokio::spawn(worker::action_worker(client, action_rx, result_tx));

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    // Show splash screen for 1 second
    terminal.draw(views::splash::draw)?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut app = App::new(action_tx, result_rx);
    app.refresh();

    let mut last_refresh = std::time::Instant::now();

    loop {
        // Check for results from background worker
        while let Ok(result) = app.result_rx.try_recv() {
            app.handle_result(result);
            last_refresh = std::time::Instant::now();
        }

        // Auto-refresh every 2 seconds
        if last_refresh.elapsed() >= Duration::from_secs(2) && !app.busy {
            app.send_action(Action::Refresh);
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
        KeyCode::Tab | KeyCode::Down => {
            if let Some(modal) = &mut app.create_modal {
                modal.focus_next();
            }
        }
        KeyCode::BackTab | KeyCode::Up => {
            if let Some(modal) = &mut app.create_modal {
                modal.focus_prev();
            }
        }
        KeyCode::Enter => {
            if let Some(modal) = &app.create_modal {
                if modal.focused_field == 10 {
                    app.submit_create();
                } else if modal.is_file_field() {
                    app.open_file_picker();
                } else if modal.is_user_data_mode_field() {
                    app.handle_user_data_mode_action();
                } else if let Some(modal) = &mut app.create_modal {
                    modal.focus_next();
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(modal) = &mut app.create_modal {
                if modal.is_boot_mode_field() {
                    modal.toggle_boot_mode();
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
    match key_code {
        KeyCode::Char('q') => app.should_quit = true,
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
