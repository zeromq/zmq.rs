use consts::ErrorCode;
use std;
use std::io::{IoError, IoErrorKind};


pub type ZmqResult<T> = Result<T, ZmqError>;

#[deriving(Show)]
pub struct ZmqError {
    pub code: ErrorCode,
    pub desc: &'static str,
    pub detail: Option<String>,
    pub iokind: Option<IoErrorKind>,
}

impl ZmqError {
    pub fn new(code: ErrorCode, desc: &'static str) -> ZmqError {
        ZmqError {
            code: code,
            desc: desc,
            detail: None,
            iokind: None,
        }
    }

    pub fn from_io_error(e: IoError) -> ZmqError {
        ZmqError {
            code: match e.kind {
                std::io::PermissionDenied => ErrorCode::EACCES,
                std::io::ConnectionRefused => ErrorCode::ECONNREFUSED,
                std::io::ConnectionReset => ErrorCode::ECONNRESET,
                std::io::ConnectionAborted => ErrorCode::ECONNABORTED,
                std::io::NotConnected => ErrorCode::ENOTCONN,
                std::io::TimedOut => ErrorCode::ETIMEDOUT,

                _ => ErrorCode::EIOERROR,
            },
            desc: e.desc,
            detail: e.detail,
            iokind: Some(e.kind),
        }
    }
}
