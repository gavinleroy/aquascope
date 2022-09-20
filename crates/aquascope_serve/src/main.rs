use serde::{Deserialize, Serialize};
use snafu::prelude::*;
use std::{env, net::SocketAddr};

const DEFAULT_ADDRESS: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8008;

mod server;

fn main() {
    // Default logging is error only, info is useful for a server.
    let env_logger_config = env_logger::Env::default().default_filter_or("info");
    env_logger::Builder::from_env(env_logger_config).init();

    // TODO setup the environment

    let cfg = Config::from_env();
    server::serve(cfg);
}

struct Config {
    address: String,
    port: u16,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorJson {
    error: String,
}

impl Config {
    fn from_env() -> Self {
        let address =
            env::var("AQUASCOPE_SERVER_ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.to_owned());
        let port = env::var("AQUASCOPE_SERVER_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        Config { address, port }
    }

    fn socket_address(&self) -> SocketAddr {
        let a = self.address.parse().expect("Invalid address");
        SocketAddr::new(a, self.port)
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SourceRequest {
    filename: String,
}

#[derive(Debug, Snafu)]
pub enum ServeError {
    #[snafu(display("An Unknown error occurred: {msg}"))]
    Unknown { msg: String },
}

pub type Result<T, E = ServeError> = ::std::result::Result<T, E>;

impl axum::response::IntoResponse for ServeError {
    fn into_response(self) -> axum::response::Response {
        let body = format!("{}", self);
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}
