use actix_files::NamedFile;
use actix_web::{get, web, App, HttpResponse};
use serde::Serialize;
use std::io::Write;
use std::sync::{Arc, Mutex};
use structopt::StructOpt;

#[derive(StructOpt)]
#[structopt(name = "sharkmon", about = "Shark 100S power meter web gateway")]
struct Opt {
    #[structopt(short, long)]
    verbose: bool,

    #[structopt(help = "IP address/hostname and port of meter, e.g., 192.168.1.100:502")]
    meter: String,

    #[structopt(
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

#[get("/power")]
async fn power(data: web::Data<Arc<Mutex<PowerEwma>>>) -> HttpResponse {
    let pe = data.lock().unwrap().clone();
    HttpResponse::Ok().json(pe)
}

pub async fn device_update(pe_mutex: Arc<Mutex<PowerEwma>>, meter: String, verbose: bool) -> ! {
    loop {
        let _ignore = device_update_connect_loop(&pe_mutex, &meter, verbose).await;
        println!("Connection error, sleeping and retrying");
        pe_mutex.lock().unwrap().update(0.0, 0.0, 0.0);
        actix_web::rt::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn device_update_connect_loop(
    pe_mutex: &Arc<Mutex<PowerEwma>>,
    meter: &str,
    verbose: bool,
) -> std::io::Result<()> {
    use tokio_modbus::prelude::*;
    let mut interval = actix_web::rt::time::interval(std::time::Duration::from_secs(1));

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
                println!("Error getting device update: {}", e);
                pe_mutex.lock().unwrap().update(0.0, 0.0, 0.0);
                return Err(e);
            }
        }
        interval.tick().await;
    }
}

#[get("/")]
pub async fn index(_req: actix_web::HttpRequest) -> actix_web::Result<NamedFile> {
    Ok(NamedFile::open("./sharkmon.html")?)
}

#[actix_web::main]
pub async fn main() -> std::io::Result<()> {
    let opt = Opt::from_args();

    let pe: Arc<Mutex<PowerEwma>> = Arc::new(Mutex::new(PowerEwma::new()));
    let peclone = pe.clone();
    if opt.no_web {
        device_update(peclone, opt.meter, true).await
    } else {
        actix_web::rt::spawn(async move { device_update(peclone, opt.meter, opt.verbose).await });
        let appdata = web::Data::new(pe);

        actix_web::HttpServer::new(move || {
            App::new()
                .app_data(appdata.clone())
                .service(power)
                .service(index)
        })
        .workers(1)
        .bind("0.0.0.0:8081")?
        .run()
        .await
    }
}
