use axum::{
    extract::Extension, http::StatusCode, routing::get, routing::get_service, Json, Router,
};
use log::{error, warn};
use serde::Serialize;
use std::io::Write;
use std::sync::{Arc, Mutex};
use clap::Parser;

#[derive(Parser)]
#[clap(name = "sharkmon", about = "Shark 100S power meter web gateway")]
struct Opt {
    #[clap(short, long)]
    verbose: bool,

    #[clap(help = "IP address/hostname and port of meter, e.g., 192.168.1.100:502")]
    meter: String,

    #[clap(
        short,
        long = "no-web",
        help = "Disable built in web server (implies verbose)"
    )]
    no_web: bool,
}

fn beu16x2_to_f32(a: &[u16]) -> f32 {
    f32::from_bits((a[0] as u32) << 16 | (a[1] as u32))
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct PowerEwma {
    #[serde(skip_serializing)]
    initialized: bool,
    pub watts: f32,
    pub volts: f32,
    pub frequency: f32,
}

const EWMA_PARAM: f32 = 0.8;
fn ewma(a: f32, b: f32, ewma: f32) -> f32 {
    a * ewma + b * (1.0 - ewma)
}

impl PowerEwma {
    fn new() -> PowerEwma {
        PowerEwma::default()
    }
    fn update(&mut self, watts: f32, volts: f32, frequency: f32) {
        if !self.initialized {
            self.watts = watts;
            self.volts = volts;
            self.frequency = frequency;
            self.initialized = true;
        } else {
            self.watts = ewma(self.watts, watts, EWMA_PARAM);
            self.volts = ewma(self.volts, volts, EWMA_PARAM);
            self.frequency = ewma(self.frequency, frequency, EWMA_PARAM);
        }
    }
}

async fn read_f32<T: tokio_modbus::client::Reader>(ctx: &mut T, loc: u16) -> std::io::Result<f32> {
    let data = ctx.read_holding_registers(loc, 2).await?;
    Ok(beu16x2_to_f32(&data))
}

const REG_WATTS: u16 = 0x383;
const REG_VOLTS: u16 = 0x03ED;
const REG_FREQ: u16 = 0x0401;

pub async fn update_pe<T: tokio_modbus::client::Reader>(
    ctx: &mut T,
    pe_mutex: &Mutex<PowerEwma>,
) -> std::io::Result<()> {
    let watts = read_f32(ctx, REG_WATTS).await?;
    let volts = read_f32(ctx, REG_VOLTS).await?;
    let frequency = read_f32(ctx, REG_FREQ).await?;
    pe_mutex.lock().unwrap().update(watts, volts, frequency);
    Ok(())
}

async fn power(Extension(data): Extension<Arc<Mutex<PowerEwma>>>) -> Json<PowerEwma> {
    Json(data.lock().unwrap().clone())
}

pub async fn device_update(pe_mutex: Arc<Mutex<PowerEwma>>, meter: String, verbose: bool) -> ! {
    loop {
        if let Err(e) = device_update_connect_loop(&pe_mutex, &meter, verbose).await {
            error!("Connection error, sleeping and retrying: {}", e);
        }
        pe_mutex.lock().unwrap().update(0.0, 0.0, 0.0);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn device_update_connect_loop(
    pe_mutex: &Arc<Mutex<PowerEwma>>,
    meter: &str,
    verbose: bool,
) -> std::io::Result<()> {
    use tokio_modbus::prelude::*;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

    let socket_addr = meter.parse().unwrap();

    let mut ctx = tcp::connect(socket_addr).await?;
    ctx.set_slave(Slave::from(1));

    loop {
        match update_pe(&mut ctx, pe_mutex).await {
            Ok(()) => {
                if verbose {
                    let pe = pe_mutex.lock().unwrap().clone();
                    std::io::stdout()
                        .write_all(serde_json::to_string(&pe).unwrap().as_bytes())
                        .expect("Could not write to stdout");
                }
            }
            Err(e) => {
                error!("Error getting device update: {}", e);
                pe_mutex.lock().unwrap().update(0.0, 0.0, 0.0);
                return Err(e);
            }
        }
        interval.tick().await;
    }
}

#[tokio::main]
pub async fn main() -> std::io::Result<()> {
    let opt = Opt::parse();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let pe = Arc::new(Mutex::new(PowerEwma::new()));
    let peclone = pe.clone();
    if opt.no_web {
        device_update(pe, opt.meter, true).await
    } else {
        tokio::spawn(async move { device_update(pe, opt.meter, opt.verbose).await });

        let app = Router::new()
            .route(
                "/",
                get_service(tower_http::services::ServeFile::new("sharkmon.html")).handle_error(
                    |error: std::io::Error| async move {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Unhandled internal error: {}", error),
                        )
                    },
                ),
            )
            .route("/power", get(power))
            .layer(Extension(peclone));

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8081));
        warn!("sharkmon starting on address {}", addr);
        if let Err(e) = axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await {
                eprintln!("Could not start server: error: {}", e);
            }
    }
    Ok(())
}
