//! HTTP/2-level forwarding proxies for the daemon services.
//!
//! Each daemon (vmm, zfs, net) exposes its gRPC services on a local TCP port.
//! When the api dials a typed gRPC client over the reverse tunnel, the request
//! lands here and is forwarded byte-for-byte to the local daemon. Streaming
//! works transparently because we forward at the HTTP/2 layer rather than
//! decoding individual messages.

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::{Request, Response, Uri};
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tonic::body::Body;
use tonic::server::NamedService;
use tower::Service;
use tracing::warn;

#[derive(Clone)]
pub struct DaemonProxy {
    upstream: Uri,
    client: Client<HttpConnector, Body>,
}

impl DaemonProxy {
    pub fn new(upstream: Uri) -> Self {
        let client = Client::builder(TokioExecutor::new())
            .http2_only(true)
            .build_http();
        Self { upstream, client }
    }

    fn rewrite_uri(&self, original: &Uri) -> Uri {
        let path_and_query = original
            .path_and_query()
            .cloned()
            .unwrap_or_else(|| "/".parse().unwrap());
        let mut parts = self.upstream.clone().into_parts();
        parts.path_and_query = Some(path_and_query);
        Uri::from_parts(parts).expect("upstream URI is valid")
    }
}

impl Service<Request<Body>> for DaemonProxy {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let new_uri = self.rewrite_uri(req.uri());
        *req.uri_mut() = new_uri;
        // The upstream is plain HTTP; strip TE and any hop-by-hop headers that
        // would confuse hyper's HTTP/2 client.
        req.headers_mut().remove(http::header::HOST);

        let client = self.client.clone();
        Box::pin(async move {
            match client.request(req).await {
                Ok(resp) => Ok(resp.map(Body::new)),
                Err(e) => {
                    warn!(error = %e, "daemon proxy upstream error");
                    Ok(grpc_error(tonic::Code::Unavailable, &e.to_string()))
                }
            }
        })
    }
}

fn grpc_error(code: tonic::Code, msg: &str) -> Response<Body> {
    Response::builder()
        .status(http::StatusCode::OK)
        .header("content-type", "application/grpc")
        .header("grpc-status", (code as i32).to_string())
        .header("grpc-message", msg)
        .body(Body::empty())
        .expect("static response is valid")
}

macro_rules! named_proxy {
    ($name:ident, $service:expr) => {
        #[derive(Clone)]
        pub struct $name(pub DaemonProxy);

        impl NamedService for $name {
            const NAME: &'static str = $service;
        }

        impl Service<Request<Body>> for $name {
            type Response = Response<Body>;
            type Error = Infallible;
            type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

            fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
                self.0.poll_ready(cx)
            }

            fn call(&mut self, req: Request<Body>) -> Self::Future {
                self.0.call(req)
            }
        }
    };
}

named_proxy!(VmServiceProxy, "mvirt.VmService");
named_proxy!(PodServiceProxy, "mvirt.PodService");
named_proxy!(ZfsServiceProxy, "mvirt.zfs.ZfsService");
named_proxy!(NetServiceProxy, "mvirt.net.NetService");
