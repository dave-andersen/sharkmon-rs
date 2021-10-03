use actix_files::NamedFile;
use actix_web::{get, web, App, HttpResponse};
use serde::Serialize;
use std::sync::{Arc, Mutex};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "sharkmon", about = "Shark 100S power meter web gateway")]
struct Opt {
    #[structopt(short, long)]
    verbose: bool,

    #[structopt(help="IP address/hostname and port of meter, e.g., 192.168.1.100:502")]
    meter: String,
}

fn beu16x2_to_f32(a: &[u16]) -> f32 {
    let as_u = (a[0] as u32) << 16 | (a[1] as u32);
    f32::from_bits(as_u)
}

#[derive(Serialize, Debug, Clone)]
pub struct PowerEwma {
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
        PowerEwma {
            initialized: false,
            watts: 0.0,
            volts: 0.0,
            frequency: 0.0,
        }
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
    {
        let mut pe = pe_mutex.lock().unwrap();
        pe.update(watts, volts, frequency);
    }
    Ok(())
}

#[get("/power")]
async fn power(data: web::Data<Arc<Mutex<PowerEwma>>>) -> actix_web::Result<HttpResponse> {
    let pe = {
        let d = data.lock().unwrap();
        d.clone()
    };
    Ok(HttpResponse::Ok().json(pe))
}

pub async fn device_update(pe_mutex: Arc<Mutex<PowerEwma>>, meter: String, verbose: bool) {
    loop {
        let _ignore = device_update_connect_loop(&pe_mutex, &meter, verbose).await;
        println!("Connection error, sleeping and retrying");
        {
            let mut pe = pe_mutex.lock().unwrap();
            pe.update(0.0, 0.0, 0.0);
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

pub async fn device_update_connect_loop(
    mut pe_mutex: &Arc<Mutex<PowerEwma>>,
    meter: &str,
    verbose: bool
) -> std::io::Result<()> {
    use tokio_modbus::prelude::*;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));

    let socket_addr = meter.parse().unwrap();

    let mut ctx = tcp::connect(socket_addr).await?;
    ctx.set_slave(Slave::from(1));

    loop {
        match update_pe(&mut ctx, &mut pe_mutex).await {
            Ok(_) => {
                if verbose {
                    let pe = pe_mutex.lock().unwrap();
                    println!("Volts: {} watts: {} frequency: {}", pe.volts, pe.watts, pe.frequency);
                }
            }
            Err(e) => {
                println!("Error getting device update: {}", e);
                let mut pe = pe_mutex.lock().unwrap();
                pe.update(0.0, 0.0, 0.0);
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
pub async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();

    let pe: Arc<Mutex<PowerEwma>> = Arc::new(Mutex::new(PowerEwma::new()));
    let peclone = pe.clone();
    tokio::spawn(async move { device_update(peclone, opt.meter, opt.verbose).await });
    let appdata = web::Data::new(pe);

    actix_web::HttpServer::new(move || {
        App::new()
            .app_data(appdata.clone())
            .service(power)
            .service(index)
    })
    .bind("0.0.0.0:8081")?
    .run()
    .await?;
    Ok(())
}
