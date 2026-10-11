//! Read-only USB discovery. Does not open handles, claim interfaces, or send HCI.
use crate::error::{AppError, Result};
use rusb::{self, Device, GlobalContext, DeviceDescriptor};

pub const BE200_VID: u16 = 0x8087;
pub const BE200_PID: u16 = 0x0036;

pub fn parse_vid_pid(s: &str) -> Result<(u16,u16)> {
    let (a,b) = s.split_once(':').ok_or_else(|| AppError::Usage(
        "Expected hexadecimal VID:PID, e.g. 8087:0036".into()))?;
    if a.len()!=4 || b.len()!=4 || !a.bytes().all(|c| c.is_ascii_hexdigit()) ||
        !b.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::Usage("VID:PID must contain exactly four hex digits on each side".into()));
    }
    Ok((u16::from_str_radix(a,16).unwrap(),u16::from_str_radix(b,16).unwrap()))
}

fn location(dev: &Device<GlobalContext>) -> String {
    let ports = dev.port_numbers().map(|p| {
        if p.is_empty() { "?".into() } else { p.iter().map(u8::to_string).collect::<Vec<_>>().join(".") }
    }).unwrap_or_else(|_| "?".into());
    format!("bus={} addr={} ports={}",dev.bus_number(),dev.address(),ports)
}
fn describe(dev:&Device<GlobalContext>, desc:&DeviceDescriptor) {
    println!("{:04x}:{:04x}  {}  usb={:04x} class={:02x}/{:02x}/{:02x} configurations={}",
        desc.vendor_id(),desc.product_id(),location(dev),
        desc.usb_version().major() as u16 * 0x100 + desc.usb_version().minor() as u16 * 0x10 + desc.usb_version().sub_minor() as u16,
        desc.class_code(),desc.sub_class_code(),desc.protocol_code(),desc.num_configurations());
}
fn devices() -> Result<rusb::DeviceList<GlobalContext>> { Ok(rusb::devices()?) }

pub fn list(target_only: bool) -> Result<()> {
    let mut count=0usize;
    for dev in devices()?.iter() {
        match dev.device_descriptor() {
            Ok(desc)=>{
                if target_only && (desc.vendor_id()!=BE200_VID || desc.product_id()!=BE200_PID) { continue; }
                describe(&dev,&desc);
                count+=1;
            }
            Err(error)=>log::debug!("descriptor read failed on {}: {error}",location(&dev)),
        }
    }
    println!("Found {count} USB device(s){}",if target_only {" matching 8087:0036"}else{""});
    Ok(())
}
fn endpoint_transfer_type(t: rusb::TransferType) -> &'static str {
    match t {
        rusb::TransferType::Control => "control",
        rusb::TransferType::Isochronous => "isochronous",
        rusb::TransferType::Bulk => "bulk",
        rusb::TransferType::Interrupt => "interrupt",
    }
}

pub fn inspect(vid:u16,pid:u16) -> Result<()> {
    let mut found=false;
    for dev in devices()?.iter() {
        let Ok(desc)=dev.device_descriptor() else { continue };
        if desc.vendor_id()!=vid || desc.product_id()!=pid {continue;}
        found=true;
        describe(&dev,&desc);
        println!("  device release: {}",desc.device_version());
        for idx in 0..desc.num_configurations() {
            match dev.config_descriptor(idx) {
                Ok(cfg)=>{
                    println!("  configuration index={} value={} max_power={}mA",
                        idx,cfg.number(),cfg.max_power());
                    for interface in cfg.interfaces() {
                        for setting in interface.descriptors() {
                            println!("    interface {} alt={} class={:02x}/{:02x}/{:02x}",
                                setting.interface_number(), setting.setting_number(),
                                setting.class_code(),setting.sub_class_code(),setting.protocol_code());
                            for ep in setting.endpoint_descriptors() {
                                println!("      ep=0x{:02x} {:?} {} max_packet={} interval={}",
                                    ep.address(),ep.direction(),endpoint_transfer_type(ep.transfer_type()),
                                    ep.max_packet_size(),ep.interval());
                            }
                        }
                    }
                }
                Err(e)=>println!("  configuration index={idx} unavailable: {e}"),
            }
        }
    }
    if !found {return Err(AppError::DeviceNotFound(vid,pid));}
    log::info!("USB descriptor inspection completed for {vid:04x}:{pid:04x}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn valid_vid_pid() {
        assert_eq!(parse_vid_pid("8087:0036").unwrap(),(0x8087,0x0036));
        assert_eq!(parse_vid_pid("0E8D:7925").unwrap(),(0x0e8d,0x7925));
    }
    #[test] fn rejects_bad_vid_pid() {
        for s in ["8087","8087:36","8087:0036:ffff","ZZZZ:0036","8087:003g"] {
            assert!(parse_vid_pid(s).is_err(),"{s}");
        }
    }
}
