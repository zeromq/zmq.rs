use crate::codec::FramedIo;
use crate::endpoint::Endpoint;
use crate::transport::AcceptStopChannel;
use crate::ZmqResult;

use std::path::PathBuf;

pub(crate) async fn connect(_path: PathBuf) -> ZmqResult<(FramedIo, Endpoint)> {
    todo!()
}
pub(crate) async fn begin_accept<T>(
    _path: PathBuf,
    _cback: impl Fn(ZmqResult<(FramedIo, Endpoint)>) -> T + Send + 'static,
) -> ZmqResult<(Endpoint, AcceptStopChannel)>
where
    T: std::future::Future<Output = ()> + Send + 'static,
{
    todo!()
}
