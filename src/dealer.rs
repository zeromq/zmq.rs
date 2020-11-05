use crate::codec::FramedIo;
use crate::ZmqResult;

pub struct DealerSocket {
    pub(crate) _inner: FramedIo,
}

impl DealerSocket {
    pub async fn bind(_endpoint: &str) -> ZmqResult<Self> {
        todo!()
    }
}
