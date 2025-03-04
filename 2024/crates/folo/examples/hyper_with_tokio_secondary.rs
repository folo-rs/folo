//! This example runs a server that responds to any request with "Hello, world!"

use std::{convert::Infallible, error::Error};

use bytes::Bytes;
use http::{header::CONTENT_TYPE, Request, Response};
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{body::Incoming, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use tokio::net::{TcpListener, TcpStream};

/// Function from an incoming request to an outgoing response
///
/// This function gets turned into a [`hyper::service::Service`] later via
/// [`service_fn`]. Instead of doing this, you could also write a type that
/// implements [`hyper::service::Service`] directly and pass that in place of
/// writing a function like this and calling [`service_fn`].
///
/// This function could use [`Full`] as the body type directly since that's
/// the only type that can be returned in this case, but this uses [`BoxBody`]
/// anyway for demonstration purposes, since this is what's usually used when
/// writing a more complex webserver library.
async fn handle_request(
    _request: Request<Incoming>,
) -> Result<Response<BoxBody<Bytes, Infallible>>, Infallible> {
    let response = Response::builder()
        .header(CONTENT_TYPE, "text/plain")
        .body(Full::new(Bytes::from_static(&[b'x'; 20480])).boxed())
        .expect("values provided to the builder should be valid");

    Ok(response)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    // We spawn a new task for the TCP dispatcher, to get it off the entrypoint thread, because
    // there are rumors that the entrypoint thread is a special thread and may behave differently.
    // This dispatcher task spawns handlers for each received connection.
    _ = tokio::spawn(dispatch_connections()).await?;

    Ok(())
}

async fn dispatch_connections() -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    let listen_addr = "0.0.0.0:1234";
    let tcp_listener = TcpListener::bind(listen_addr).await?;
    println!("Listening on http://{listen_addr}");

    loop {
        let (stream, _) = match tcp_listener.accept().await {
            Ok(x) => x,
            Err(_) => {
                continue;
            }
        };

        tokio::spawn(accept_connection(stream));
    }
}

async fn accept_connection(stream: TcpStream) {
    let connection = Builder::new(TokioExecutor::new());
    let io = TokioIo::new(stream);
    _ = connection
        .serve_connection(io, service_fn(handle_request))
        .await;
}
