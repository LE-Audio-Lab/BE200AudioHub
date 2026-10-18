use crossbeam_channel::{Sender, TrySendError};

pub use crate::plane::l1_hci::iso::tx::LogicalIsoSduGroupRequest;

use crate::{
    plane::l1_hci::iso::tx::{
        IsoDataGroupRequest, IsoSduGroupRequest,
    },
    runtime::mailbox::SendError,
};

#[derive(Clone)]
pub struct HciIsoTx {
    tx: Sender<IsoDataGroupRequest>,
}

#[derive(Clone)]
pub struct HciLogicalIsoTx {
    tx: Sender<IsoDataGroupRequest>,
}

impl HciIsoTx {
    pub(crate) fn new(tx: Sender<IsoDataGroupRequest>) -> Self {
        Self { tx }
    }


    pub fn try_submit_group(
        &self,
        request: IsoSduGroupRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(IsoDataGroupRequest::Raw(request))
            .map_err(map_send_error)
    }
}

impl HciLogicalIsoTx {
    pub(crate) fn new(tx: Sender<IsoDataGroupRequest>) -> Self {
        Self { tx }
    }

    pub fn try_submit_group(
        &self,
        request: LogicalIsoSduGroupRequest,
    ) -> Result<(), SendError> {
        self.tx
            .try_send(IsoDataGroupRequest::Logical(request))
            .map_err(map_send_error)
    }
}


fn map_send_error<T>(error: TrySendError<T>) -> SendError {
    match error {
        TrySendError::Full(_) => SendError::Full,
        TrySendError::Disconnected(_) => SendError::Disconnected,
    }
}
