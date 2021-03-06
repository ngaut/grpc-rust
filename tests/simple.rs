extern crate grpc;
extern crate futures;

mod test_misc;

use std::sync::Arc;

use futures::*;
use futures::stream::Stream;

use grpc::server::*;
use grpc::client::*;
use grpc::method::*;
use grpc::marshall::*;
use grpc::error::*;
use grpc::futures_grpc::*;

use test_misc::TestSync;


fn string_string_method(name: &str, streaming: GrpcStreaming)
    -> Arc<MethodDescriptor<String, String>>
{
    Arc::new(MethodDescriptor {
       name: name.to_owned(),
       streaming: streaming,
       req_marshaller: Box::new(MarshallerString),
       resp_marshaller: Box::new(MarshallerString),
   })
}

/// Single method server on random port
fn new_server<H>(name: &str, streaming: GrpcStreaming, handler: H) -> GrpcServer
    where H : MethodHandler<String, String> + 'static + Sync + Send
{
    let mut methods = Vec::new();
    methods.push(ServerMethod::new(
        string_string_method(name, streaming),
        handler,
    ));
    GrpcServer::new(0, ServerServiceDefinition::new(methods))
}

/// Single unary method server
fn new_server_unary<H>(name: &str, handler: H) -> GrpcServer
    where H : Fn(String) -> GrpcFutureSend<String> + Sync + Send + 'static
{
    new_server(name, GrpcStreaming::Unary, MethodHandlerUnary::new(handler))
}

/// Single server streaming method server
fn new_server_server_streaming<H>(name: &str, handler: H) -> GrpcServer
    where H : Fn(String) -> GrpcStreamSend<String> + Sync + Send + 'static
{
    new_server(name, GrpcStreaming::ServerStreaming, MethodHandlerServerStreaming::new(handler))
}


/// Tester for unary methods
struct TesterUnary {
    name: String,
    client: GrpcClient,
    _server: GrpcServer,
}

impl TesterUnary {
    fn new<H>(handler: H) -> TesterUnary
        where H : Fn(String) -> GrpcFutureSend<String> + Sync + Send + 'static
    {
        let name = "/text/Unary";
        let server = new_server_unary(name, handler);
        let port = server.local_addr().port();
        TesterUnary {
            name: name.to_owned(),
            _server: server,
            client: GrpcClient::new("::1", port).unwrap()
        }
    }

    fn call(&self, param: &str) -> GrpcFutureSend<String> {
        self.client.call_unary(param.to_owned(), string_string_method(&self.name, GrpcStreaming::Unary))
    }

    fn call_expect_error<F : FnOnce(&GrpcError) -> bool>(&self, param: &str, expect: F) {
        match self.call(param).wait() {
            Ok(r) => panic!("expecting error, got: {:?}", r),
            Err(e) => assert!(expect(&e), "wrong error: {:?}", e),
        }
    }

    fn call_expect_grpc_error<F : FnOnce(&str) -> bool>(&self, param: &str, expect: F) {
        self.call_expect_error(param, |e| {
            match e {
                &GrpcError::GrpcMessage(GrpcMessageError { ref grpc_message }) if expect(&grpc_message) => true,
                _ => false,
            }
        });
    }

    fn call_expect_grpc_error_contain(&self, param: &str, expect: &str) {
        self.call_expect_grpc_error(param, |m| m.find(expect).is_some());
    }
}


/// Tester for server streaming methods
struct TesterServerStreaming {
    name: String,
    client: GrpcClient,
    _server: GrpcServer,
}

impl TesterServerStreaming {
    fn new<H>(handler: H) -> TesterServerStreaming
        where H : Fn(String) -> GrpcStreamSend<String> + Sync + Send + 'static
    {
        let name = "/test/ServerStreaming";
        let server = new_server_server_streaming(name, handler);
        let port = server.local_addr().port();
        TesterServerStreaming {
            name: name.to_owned(),
            _server: server,
            client: GrpcClient::new("::1", port).unwrap()
        }
    }

    fn call(&self, param: &str) -> GrpcStream<String> {
        self.client.call_server_streaming(param.to_owned(), string_string_method(&self.name, GrpcStreaming::ServerStreaming))
    }
}

#[test]
fn unary() {
    let tester = TesterUnary::new(|s| Box::new(Ok(s).into_future()));

    assert_eq!("aa", tester.call("aa").wait().unwrap());
}

#[test]
fn server_is_not_running() {
    let client = GrpcClient::new("::1", 2).unwrap();

    // TODO: https://github.com/tokio-rs/tokio-core/issues/12
    if false {
        let result = client.call_unary("aa".to_owned(), string_string_method("/does/not/matter", GrpcStreaming::Unary)).wait();
        assert!(result.is_err(), result);
    }
}

#[test]
fn error_in_handler() {
    let tester = TesterUnary::new(|_| Box::new(Err(GrpcError::Other("my error")).into_future()));

    tester.call_expect_grpc_error_contain("aa", "my error");
}

// TODO
//#[test]
fn panic_in_handler() {
    let tester = TesterUnary::new(|_| panic!("icnap"));

    tester.call_expect_grpc_error("aa",
        |m| m.find("Panic").is_some() && m.find("icnap").is_some());
}

#[test]
fn server_streaming() {
    let test_sync = Arc::new(TestSync::new());
    let test_sync_server = test_sync.clone();

    let tester = TesterServerStreaming::new(move |s| {
        let sync = test_sync_server.clone();
        test_misc::stream_thread_spawn_iter(move || {
            (0..3).map(move |i| {
                sync.take(i * 2 + 1);
                format!("{}{}", s, i)
            })
        })
    });

    let mut rs = tester.call("x").wait();

    test_sync.take(0);
    assert_eq!("x0", rs.next().unwrap().unwrap());
    test_sync.take(2);
    assert_eq!("x1", rs.next().unwrap().unwrap());
    test_sync.take(4);
    assert_eq!("x2", rs.next().unwrap().unwrap());
    assert!(rs.next().is_none());
}
