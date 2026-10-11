use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Usage(String),
    Usb(rusb::Error),
    DeviceNotFound(u16, u16),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(s) => write!(f, "{s}"),
            Self::Usb(e) => write!(f, "USB error: {e}"),
            Self::DeviceNotFound(vid, pid) => {
                write!(f, "Device {vid:04x}:{pid:04x} not found")
            }
        }
    }
}

impl std::error::Error for AppError {}

impl From<rusb::Error> for AppError {
    fn from(value: rusb::Error) -> Self {
        Self::Usb(value)
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
