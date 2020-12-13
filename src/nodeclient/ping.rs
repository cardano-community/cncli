use std::io::Write;
use std::time::{Duration, Instant};

use cardano_ouroboros_network::mux;
use futures::executor::block_on;

pub fn ping<W: Write>(out: &mut W, host: &String, port: u16, network_magic: u32) {
    block_on(async {
        let start = Instant::now();
        match mux::tcp::connect(&host.as_str(), port, network_magic).await {
            Ok((channel, connect_duration)) => {
                // TODO: Once implemented, channel.shutdown(); gracefully
                let total_duration = start.elapsed();
                ping_json_success(out, connect_duration, total_duration, host, port);
            }
            Err(error) => {
                ping_json_error(out, format!("{}", error), host, port);
            }
        }
    });
}

fn ping_json_success<W: Write>(out: &mut W, connect_duration: Duration, total_duration: Duration, host: &String, port: u16) {
    write!(out, "{{\n\
        \x20\"status\": \"ok\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"connectDurationMs\": {},\n\
        \x20\"durationMs\": {}\n\
    }}", host, port, connect_duration.as_millis(), total_duration.as_millis()).unwrap();
}

fn ping_json_error<W: Write>(out: &mut W, message: String, host: &String, port: u16) {
    write!(out, "{{\n\
        \x20\"status\": \"error\",\n\
        \x20\"host\": \"{}\",\n\
        \x20\"port\": {},\n\
        \x20\"errorMessage\": \"{}\"\n\
    }}", host, port, message).unwrap();
}