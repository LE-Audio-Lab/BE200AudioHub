pub const MAX_HCI_CONNECTION_HANDLE: u16 = 0x0EFF;
pub const MAX_ISO_SDU_LENGTH: usize = 0x0FFF;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IsoSduRequest {
    pub connection_handle: u16,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsoSduConfigError {
    InvalidConnectionHandle(u16),
    SduTooLarge { length: usize },
}

impl IsoSduRequest {
    pub fn validate(&self) -> Result<(), IsoSduConfigError> {
        if self.connection_handle > MAX_HCI_CONNECTION_HANDLE {
            return Err(IsoSduConfigError::InvalidConnectionHandle(
                self.connection_handle,
            ));
        }
        if self.payload.len() > MAX_ISO_SDU_LENGTH {
            return Err(IsoSduConfigError::SduTooLarge {
                length: self.payload.len(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_sdu_boundary_accepts_generic_payload_sizes() {
        IsoSduRequest {
            connection_handle: 0x0900,
            payload: vec![],
        }
        .validate()
        .unwrap();
        IsoSduRequest {
            connection_handle: 0x0900,
            payload: vec![0; MAX_ISO_SDU_LENGTH],
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn complete_sdu_boundary_rejects_invalid_handle_and_oversize() {
        assert_eq!(
            IsoSduRequest {
                connection_handle: 0x0F00,
                payload: vec![1],
            }
            .validate(),
            Err(IsoSduConfigError::InvalidConnectionHandle(0x0F00))
        );
        assert_eq!(
            IsoSduRequest {
                connection_handle: 0x0900,
                payload: vec![0; MAX_ISO_SDU_LENGTH + 1],
            }
            .validate(),
            Err(IsoSduConfigError::SduTooLarge {
                length: MAX_ISO_SDU_LENGTH + 1,
            })
        );
    }
}
