//! This example runs a server that responds to any request with "Hello, world!"

use bytes::Bytes;
use hello_world::{
    greeter_server::{Greeter, GreeterServer},
    HelloReply, HelloRequest,
};
use http::{Request, Response};
use http_body_util::combinators::UnsyncBoxBody;
use hyper::{body::Incoming, service::service_fn};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
};
use std::{convert::Infallible, error::Error};
use tokio::net::{TcpListener, TcpStream};
use tonic::{client::GrpcService, Status};

pub mod hello_world {
    tonic::include_proto!("greet");
}

/// Function from an incoming request to an outgoing response
async fn handle_request(
    request: Request<Incoming>,
) -> Result<Response<UnsyncBoxBody<Bytes, Status>>, Infallible> {
    // Let's create the GreeterServer that will handle this request with gRPC handling.
    let mut server = GreeterServer::new(MyGreeter::new());
    match server.call(request).await {
        Ok(res) => Ok(res),
        Err(_) => unreachable!(),
    }
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

#[derive(Default)]
pub struct MyGreeter {}

impl MyGreeter {
    pub fn new() -> Self {
        MyGreeter {}
    }
}

// We need to implement the behavior of the Greeter service
#[tonic::async_trait]
impl Greeter for MyGreeter {
    async fn say_hello(
        &self,
        request: tonic::Request<HelloRequest>,
    ) -> Result<tonic::Response<HelloReply>, Status> {
        let reply = hello_world::HelloReply {
            message: format!("Hello {}!", request.into_inner().name),
        };
        Ok(tonic::Response::new(reply))
    }
}
