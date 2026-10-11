mod error;
mod logging;
mod usb;

use error::{AppError, Result};
use std::env;

fn usage()->&'static str {
    "BE200AudioHub v0.1 (R1.1/R1.2 read-only)\n\
     Usage:\n\
       be200hub doctor\n\
       be200hub usb list [--target-only]\n\
       be200hub usb inspect [VID:PID]\n\
       be200hub help\n\
     BE200 default target: 8087:0036\n\
     Logging: %LOCALAPPDATA%\\BE200AudioHub\\logs\\be200hub.log; BE200_LOG=debug"
}
fn doctor()->Result<()>{
    println!("BE200AudioHub 0.1.0 — read-only diagnostics");
    println!("OS: {}",env::consts::OS);
    println!("Architecture: {}",env::consts::ARCH);
    println!("Target: {:04x}:{:04x}",usb::BE200_VID,usb::BE200_PID);
    println!("USB backend: libusb/rusb (vendored)");
    println!("Driver ownership: unchanged (R1.3 not implemented)");
    println!("Log directory: {}",logging::default_directory().display());
    println!("BE200 status:");
    usb::list(true)
}
fn run(args:&[String])->Result<()> {
    match args {
        [x] if x=="help"||x=="--help"||x=="-h" => {println!("{}",usage());Ok(())}
        [x] if x=="doctor" => doctor(),
        [a,b] if a=="usb"&&b=="list" =>usb::list(false),
        [a,b,c] if a=="usb"&&b=="list"&&c=="--target-only" =>usb::list(true),
        [a,b] if a=="usb"&&b=="inspect" =>usb::inspect(usb::BE200_VID,usb::BE200_PID),
        [a,b,c] if a=="usb"&&b=="inspect" =>{
            let (vid,pid)=usb::parse_vid_pid(c)?;usb::inspect(vid,pid)
        }
        _=>Err(AppError::Usage(usage().into())),
    }
}
fn main(){
    let logger=match logging::install(&logging::default_directory()){
        Ok(runtime)=>Some(runtime),
        Err(e)=>{eprintln!("Warning: file logging unavailable: {e}");None}
    };
    let args:Vec<String>=env::args().skip(1).collect();
    log::info!("BE200AudioHub invoked: {:?}",args);
    match run(&args){
        Ok(())=>log::info!("command completed successfully"),
        Err(e)=>{
            eprintln!("Error: {e}");
            log::error!("{e}");
            if let Some(logger)=logger.as_ref(){logger.flush();}
            std::process::exit(1);
        }
    }
    if let Some(logger)=logger.as_ref(){
        logger.flush();
        log::debug!("Log file: {}",logger.path().display());
    }
}
