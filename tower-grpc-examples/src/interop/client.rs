
extern crate console;
#[macro_use]
extern crate clap;
extern crate env_logger;
extern crate http;
extern crate futures;
#[macro_use]
extern crate log;
extern crate prost;
#[macro_use]
extern crate prost_derive;
extern crate tokio_core;
extern crate rustls;
extern crate tower;
extern crate tower_h2;
extern crate tower_grpc;

use std::io::Error as IoError;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use http::header::HeaderValue;
use futures::{future, Future, stream, Stream};
use tokio_core::reactor::Core;
use tokio_core::net::TcpStream;
use tower_grpc::{Request, Response};
use tower_h2::client::Connection;

use pb::SimpleRequest;
use pb::client::TestService;


mod pb {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/grpc.testing.rs"));
}

mod util;

const LARGE_REQ_SIZE: usize = 271828;
const LARGE_RSP_SIZE: i32 = 314159;

arg_enum!{
    #[derive(Debug, Copy, Clone)]
    #[allow(non_camel_case_types)]
    enum Testcase {
        empty_unary,
        cacheable_unary,
        large_unary,
        client_compressed_unary,
        server_compressed_unary,
        client_streaming,
        client_compressed_streaming,
        server_streaming,
        server_compressed_streaming,
        ping_pong,
        empty_stream,
        compute_engine_creds,
        jwt_token_creds,
        oauth2_auth_token,
        per_rpc_creds,
        custom_metadata,
        status_code_and_message,
        unimplemented_method,
        unimplemented_service,
        cancel_after_begin,
        cancel_after_first_response,
        timeout_on_sleeping_server,
        concurrent_large_unary
    }
}

macro_rules! test_assert {
    ($description:expr, $assertion:expr) => {
        if $assertion {
            TestAssertion::Passed { description: $description }
        } else {
            TestAssertion::Failed { 
                description: $description,
                expression: stringify!($assertion),
                why: None
            }
        }
    };
    ($description:expr, $assertion:expr, $why:expr) => {
        if $assertion {
            TestAssertion::Passed { description: $description }
        } else {
            TestAssertion::Failed { 
                description: $description,
                expression: stringify!($assertion),
                why: Some($why)
            }
        }
    }; 
}

// pub struct TestResults {
//     name: String, 
//     assertions: Vec<TestAssertion>,
// }

// impl TestResults {
//     pub fn passed(&self) -> bool {
//         self.assertions.iter().all(TestAssertion::passed)
//     }
// }

// impl fmt::Display for TestResults {
//     fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//         use console::{Emoji, style};
//         let passed = self.is_passed();
//         write!(f, "{check} {name}\n",
//             check = if passed { 
//                 style(Emoji("✔", "+")).green()
//             } else {
//                 style(Emoji("✖", "x")).red()
//             },
//             name = if passed { 
//                 style(self.name).green()
//             } else {
//                 style(self.name).red()
//             },
//         )?;
//         for result in self.assertions {
//             write!(f, "  {}\n", result)?;
//         }
//     }
// }

impl Testcase {
    fn run(&self, server: &ServerInfo, core: &mut tokio_core::reactor::Core) 
           -> Result<Vec<TestAssertion>, Box<Error>> {
        
        let reactor = core.handle();
        let mut client = core.run(
            TcpStream::connect(&server.addr, &reactor)
                .and_then(move |socket| {
                    // Bind the HTTP/2.0 connection
                    Connection::handshake(socket, reactor)
                        .map_err(|_| panic!("failed HTTP/2.0 handshake"))
                })
                .and_then(move |conn| {
                Ok(TestService::new(conn, server.uri.clone())
                        .expect("TestService::new"))
                })
        ).expect("client");
            
        match *self {
            Testcase::empty_unary => {
                use pb::Empty;
                core.run(client.empty_call(Request::new(Empty {}))
                    .then(|result| {
                        let mut assertions = vec![
                            test_assert!(
                                "call must be successful",
                                result.is_ok(),
                                format!("result={:?}", result)
                            )
                        ];
                        if let Ok(body) = result.map(|r| r.into_inner()) {
                            assertions.push(test_assert!(
                                "body must not be null",
                                body == Empty{},
                                format!("body={:?}", body)
                            ))
                        }
                        future::ok::<Vec<TestAssertion>, Box<Error>>(assertions)
                    }))
            },
            Testcase::large_unary => {
                use std::mem;
                let payload = util::client_payload(LARGE_REQ_SIZE);
                let req = SimpleRequest {
                    response_type: pb::PayloadType::Compressable as i32,
                    response_size: LARGE_RSP_SIZE,
                    payload: Some(payload),
                    ..Default::default()
                };
                core.run(client.unary_call(Request::new(req))
                    .then(|result| {
                        println!("received {:?}", result);
                    let mut assertions = vec![
                            test_assert!(
                                "call must be successful",
                                result.is_ok(),
                                format!("result={:?}", result)
                            )
                    ];
                        if let Ok(body) = result.map(|r| r.into_inner()) {
                            assertions.push(test_assert!(
                            "body must be 314159 bytes",
                            mem::size_of_val(&body) == LARGE_RSP_SIZE as usize,
                            format!("mem::size_of_val(&body)={:?}", 
                                mem::size_of_val(&body))
                            ));
                        }
                        future::ok::<Vec<TestAssertion>, Box<Error>>(assertions)
                    }))
            },
            Testcase::cacheable_unary => {
                let payload = pb::Payload {
                    type_: pb::PayloadType::Compressable as i32,
                    body: format!("{:?}", std::time::Instant::now()).into_bytes(),
                };
                let req = SimpleRequest {
                    response_type: pb::PayloadType::Compressable as i32,
                    payload: Some(payload),
                    ..Default::default()
                };
                let mut req = Request::new(req);
                req.headers_mut()
                    .insert(" x-user-ip", HeaderValue::from_static("1.2.3.4"));
                // core.run(client.unary_call(req)
                //     .then(|result| { 
                //         unimplemented!()
                //     })
                // )
                unimplemented!()
            },
            Testcase::client_streaming => {
                let stream = stream::iter_ok(vec![
                    util::client_payload(27182),
                    util::client_payload(8),
                    util::client_payload(1828),
                    util::client_payload(45904),
                ]);
                core.run(
                    client.streaming_input_call(Request::new(stream))
                        .then(|result| {
                            let mut assertions = vec![
                                    test_assert!(
                                        "call must be successful",
                                        result.is_ok(),
                                        format!("result={:?}", result)
                                    )
                            ];
                            if let Ok(response) = result.map(|r| r.into_inner()) {
                                assertions.push(test_assert!(
                                "aggregated payload size must be 74922 bytes",
                                response.aggregated_payload_size == 74922,
                                format!("aggregated_payload_size={:?}", 
                                    response.aggregated_payload_size
                                )));
                            }
                            future::ok::<Vec<TestAssertion>, Box<Error>>(assertions)
                        })
                )
            },
            Testcase::compute_engine_creds | Testcase::jwt_token_creds | 
                Testcase::oauth2_auth_token | Testcase::per_rpc_creds => 
                unimplemented!("test case unimplemented: tower-grpc does not currently support auth."),        
            _ => unimplemented!()
        }
    }
}
enum TestAssertion {
    Passed { description: &'static str },
    Failed { description: &'static str, 
             expression: &'static str, 
             why: Option<String> },
    Errored { description: &'static str, error: Box<Error> }
}

impl TestAssertion {
    fn passed(&self) -> bool {
        if let TestAssertion::Passed { .. } = *self {
            true
        } else {
            false
        }
    }
}

impl fmt::Display for TestAssertion {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use console::{Emoji, style};
        match *self {
            TestAssertion::Passed { ref description } =>
                write!(f, "{check} {desc}", 
                    check = style(Emoji("✔", "+")).green(),
                    desc = style(description).green(),
                ),
            TestAssertion::Failed { 
                ref description,
                ref expression,
                why: Some(ref why),
            } =>
                write!(f, "{check} {desc}\n  in `{exp}`: {why}",
                    check = style(Emoji("✖", "x")).red(),
                    desc = style(description).red(),
                    exp = style(expression).red(),
                    why = style(why).red(),
                ),
            TestAssertion::Failed { 
                ref description,
                ref expression,
                why: None,
            } =>
                write!(f, "{check} {desc}\n  in `{exp}`",
                    check = style(Emoji("✖", "x")).red(),
                    desc = style(description).red(),
                    exp = style(expression).red(),
                ),
            _ => unimplemented!()
        }
        
    }
}

// impl Testcase {
//     fn future<S>(self, client: &mut pb::client::TestService<S>) -> TestFuture
//     where 
//         S: tower_h2::HttpService,
//         S::Future: Future,
//     {

//     }
        
// }

struct ServerInfo {
    addr: SocketAddr,
    uri: http::Uri,
    hostname_override: Option<String>,
}

impl<'a> From<&'a clap::ArgMatches<'a>> for ServerInfo {
    fn from(matches: &'a clap::ArgMatches<'a>) -> Self {
        let ip = value_t!(matches, "server_host", IpAddr)
            .unwrap_or_else(|e| e.exit());
        let port = value_t!(matches, "server_port", u16)
            .unwrap_or_else(|e| e.exit());

        let addr = SocketAddr::new(ip, port);
        info!("server_address={:?};", addr);

        let ip_str = matches
            .value_of("server_host")
            .expect("server_host was None unexpected!")
            ;
        let port_str = matches
            .value_of("server_port")
            .expect("server_port was None unexpectedly!")
            ;
        let uri: http::Uri = format!("http://{}:{}", ip_str, port_str)
            .parse()
            .expect("invalid uri")
            ;

        ServerInfo {
            addr,
            uri,
            hostname_override: None, // unimplemented
        }
    }
}

fn main() {
    use clap::{Arg, App};
    let _ = ::env_logger::init();

    let matches = 
        App::new("interop-client")
            .author("Eliza Weisman <eliza@buoyant.io>")
            .arg(Arg::with_name("server_host")
                .long("server_host")
                .value_name("HOSTNAME")
                .help("The server host to connect to. For example, \"localhost\" or \"127.0.0.1\"")
                .takes_value(true)
                .default_value("127.0.0.1")
            )
            .arg(Arg::with_name("server_host_override")
                .long("server_host_override")
                .value_name("HOSTNAME")
                .help("The server host to claim to be connecting to, for use in TLS and HTTP/2 :authority header. If unspecified, the value of `--server_host` will be used")
                .takes_value(true)
            )
            .arg(Arg::with_name("server_port")
                .long("server_port")
                .value_name("PORT")
                .help("The server port to connect to. For example, \"8080\".")
                .takes_value(true)
                .default_value("10000")
            )
            .arg(Arg::with_name("test_case")
                .long("test_case")
                .value_name("TESTCASE")
                .help("The name of the test case to execute. For example, 
                \"empty_unary\".")
                .possible_values(&Testcase::variants())
                .default_value("large_unary")
                .takes_value(true)
                .min_values(1)
                .use_delimiter(true)
            )
            .arg(Arg::with_name("use_tls")
                .long("use_tls")
                .help("Whether to use a plaintext or encrypted connection.")
                .takes_value(true)
                .value_name("BOOLEAN")
                .possible_values(&["true", "false"])
                .default_value("false")
                .validator(|s| 
                    // use a Clap validator for unimplemented flags so we get a 
                    // nicer error message than the panic from 
                    // `unimplemented!()`.
                    if s == "true" {
                        // unsupported, always error for now.
                        Err(String::from(
                            "tower-grpc does not currently support TLS."
                        ))
                    } else {
                        Ok(())
                    }
   
                )
            )
            .arg(Arg::with_name("use_test_ca")
                .long("use_test_ca")
                .help("Whether to replace platform root CAs with ca.pem as the CA root.")
            )
            .arg(Arg::with_name("ca_file")
                .long("ca_file")
                .value_name("FILE")
                .help("The file containing the CA root cert file")
                .takes_value(true)
                .default_value("ca.pem")
            )
            .arg(Arg::with_name("oauth_scope")
                .long("oauth_scope")
                .value_name("SCOPE")
                .help("The scope for OAuth2 tokens. For example, \"https://www.googleapis.com/auth/xapi.zoo\".")
                .takes_value(true)
                .validator(|_| 
                    // unsupported, always error for now.
                    Err(String::from(
                        "tower-grpc does not currently support GCE auth."
                    ))
                )
            )
            .arg(Arg::with_name("default_service_account")
                .long("default_service_account")
                .value_name("ACCOUNT_EMAIL")
                .help("Email of the GCE default service account.")
                .takes_value(true)
                .validator(|_| 
                    // unsupported, always error for now.
                    Err(String::from(
                        "tower-grpc does not currently support GCE auth."
                    ))
                )
            )
            .arg(Arg::with_name("service_account_key_file")
                .long("service_account_key_file")
                .value_name("PATH")
                .help("The path to the service account JSON key file generated from GCE developer console.")
                .takes_value(true)
                .validator(|_| 
                    // unsupported, always error for now.
                    Err(String::from(
                        "tower-grpc does not currently support GCE auth."
                    ))
                )
            )
            .get_matches();

    if matches.is_present("oauth_scope") || 
       matches.is_present("default_service_account") ||
       matches.is_present("service_account_key_file") {
        unimplemented!("tower-grpc does not currently support GCE auth.");
    }

    let server = ServerInfo::from(&matches);
    let test_cases = values_t!(matches, "test_case", Testcase)
        .unwrap_or_else(|e| e.exit());

    let mut core = Core::new().expect("could not create reactor core!");
    
    for test in test_cases {
        println!("{:?}:", test);
        let test_results = test
            .run(&server, &mut core)
            .expect("error running test!");
        for result in test_results {
            println!("  {}", result);
        }
    }

    

    // match test_case {
    //     Testcase::empty_unary => {
    //         let test = 
    //         core.run(test).expect("run test");
    //     },
    //     // cacheable_unary => {                
    //     //     let test = 
    //     //         TcpStream::connect(&addr, &reactor)
    //     //             .and_then(move |socket| {
    //     //                 // Bind the HTTP/2.0 connection
    //     //                 Connection::handshake(socket, reactor)
    //     //                     .map_err(|_| panic!("failed HTTP/2.0 handshake"))
    //     //             })
    //     //             .and_then(move |conn| {
    //     //                 use testing::client::TestService;
    //     //                 let client = TestService::new(conn, uri)
    //     //                     .expect("TestService::new");
    //     //                 Ok(client)
    //     //             })
    //     //             .and_then(|mut client| {
    //     //                 use testing::SimpleRequest;

    //     //                 client.cacheable_unary(Request::new(SimpleRequest {
                            
    //     //                 }))
    //     //                     .map_err(|e| panic!("gRPC request failed; err={:?}", e))
    //     //             })
    //     //             .and_then(|response| {
    //     //                 println!("RESPONSE = {:?}", response);
    //     //                 Ok(())
    //     //             })
    //     //             .map_err(|e| {
    //     //                 println!("ERR = {:?}", e);
    //     //             });
    //     //     core.run(test).expect("run test");

    //     // },
    //     Testcase::large_unary => {
    //         let test = 
    //             TcpStream::connect(&addr, &reactor)
    //                 .and_then(move |socket| {
    //                     // Bind the HTTP/2.0 connection
    //                     Connection::handshake(socket, reactor)
    //                         .map_err(|_| panic!("failed HTTP/2.0 handshake"))
    //                 })
    //                 .and_then(move |conn| {
    //                     use pb::client::TestService;
    //                     let client = TestService::new(conn, uri)
    //                         .expect("TestService::new");
    //                     Ok(client)
    //                 })
    //                 .and_then(|mut client| {
    //                     use pb::SimpleRequest;
    //                     let payload = util::client_payload(
    //                         pb::PayloadType::Compressable,
    //                         LARGE_REQ_SIZE,
    //                     );
    //                     let req = SimpleRequest {
    //                         response_type: pb::PayloadType::Compressable as i32,
    //                         response_size: LARGE_RSP_SIZE,
    //                         payload: Some(payload),
    //                         ..Default::default()
    //                     };
    //                     client.unary_call(Request::new(req))
    //                         .map_err(|e| panic!("gRPC request failed; err={:?}", e))
    //                 })
    //                 .and_then(|response| {
    //                     println!("RESPONSE = {:?}", response);
    //                     Ok(())
    //                 })
    //                 .map_err(|e| {
    //                     println!("ERR = {:?}", e);
    //                 });
    //         core.run(test).expect("run test");
    //     },
    //     t => unimplemented!("test case {:?} is not yet implemented.", t),
    // };


}