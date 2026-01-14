use clap::Parser;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{transport::Server, Request, Response, Status};
use tracing::{error, info};

pub mod mvirt {
    pub mod log {
        tonic::include_proto!("mvirt.log");
    }
}

use mvirt::log::log_service_server::{LogService, LogServiceServer};
use mvirt::log::{LogEntry, LogRequest, LogResponse, QueryRequest};

mod storage;
use std::sync::Arc;
use storage::LogManager;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value_t = 50052)]
    port: u16,

    /// Data directory for logs
    #[arg(long, default_value = "/var/lib/mvirt/logs")]
    data_dir: PathBuf,
}

pub struct MyLogService {
    manager: Arc<LogManager>,
}

#[tonic::async_trait]
impl LogService for MyLogService {
    async fn log(&self, request: Request<LogRequest>) -> Result<Response<LogResponse>, Status> {
        let req = request.into_inner();
        let entry = req
            .entry
            .ok_or_else(|| Status::invalid_argument("Missing entry"))?;

        match self.manager.append(entry) {
            Ok(id) => Ok(Response::new(LogResponse { id })),
            Err(e) => {
                error!("Failed to write log: {:?}", e);
                Err(Status::internal("Storage error"))
            }
        }
    }

    type QueryStream = ReceiverStream<Result<LogEntry, Status>>;

    async fn query(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<Self::QueryStream>, Status> {
        let req = request.into_inner();
        let (tx, rx) = mpsc::channel(64);
        let manager = self.manager.clone();

        tokio::spawn(async move {
            let limit = if req.limit == 0 {
                100
            } else {
                req.limit as usize
            };

            // Map optional fields
            let start = req.start_time_ns;
            let end = req.end_time_ns;

            // Handle object_id properly: if it's Some("") treat as None? Or just pass through.
            let obj = match req.object_id {
                Some(o) if o.is_empty() => None,
                Some(o) => Some(o),
                None => None,
            };

            let result =
                tokio::task::spawn_blocking(move || manager.query(obj, start, end, limit)).await;

            match result {
                Ok(Ok(logs)) => {
                    for log in logs {
                        if tx.send(Ok(log)).await.is_err() {
                            break;
                        }
                    }
                }
                Ok(Err(e)) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Query failed: {}", e))))
                        .await;
                }
                Err(e) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Task join error: {}", e))))
                        .await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Ensure data directory exists
    std::fs::create_dir_all(&args.data_dir)?;

    info!("Opening log storage at {:?}", args.data_dir);
    let manager = Arc::new(LogManager::new(&args.data_dir)?);

    let addr = format!("0.0.0.0:{}", args.port).parse()?;
    let service = MyLogService { manager };

    info!("mvirt-log listening on {}", addr);

    Server::builder()
        .add_service(LogServiceServer::new(service))
        .serve(addr)
        .await?;

    Ok(())
}
