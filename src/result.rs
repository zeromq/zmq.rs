use consts;


pub type ZmqResult<T> = Result<T, ZmqError>;

pub struct ZmqError {
    pub code: consts::ErrorCode,
    pub desc: &'static str,
    pub detail: Option<~str>,
}

