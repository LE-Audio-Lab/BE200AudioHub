mod architecture;
mod runtime;
mod platform;
mod transport;

#[cfg(any(feature = "hci-wip", test))]
mod hci_host;

fn main() {
    println!("BE200AudioHub Review/Refactor architecture scaffold");
}
