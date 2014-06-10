use consts;
use std;
use std::io::{IoResult, IoError, IoErrorKind};


pub type ZmqResult<T> = Result<T, ZmqError>;

pub struct ZmqError {
    pub code: consts::ErrorCode,
    pub desc: &'static str,
    pub detail: Option<String>,
    pub iokind: Option<IoErrorKind>,
}

impl ZmqError {
    pub fn new(code: consts::ErrorCode, desc: &'static str) -> ZmqError {
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
                std::io::PermissionDenied => consts::EACCES,
                std::io::ConnectionRefused => consts::ECONNREFUSED,
                std::io::ConnectionReset => consts::ECONNRESET,
                std::io::ConnectionAborted => consts::ECONNABORTED,
                std::io::NotConnected => consts::ENOTCONN,
                std::io::TimedOut => consts::ETIMEDOUT,

                _ => consts::EIOERROR,
            },
            desc: e.desc,
            detail: e.detail,
            iokind: Some(e.kind),
        }
    }

    pub fn wrap_io_error<T>(result: IoResult<T>) -> ZmqResult<T> {
        match result {
            Ok(val) => Ok(val),
            Err(e) => Err(ZmqError::from_io_error(e)),
        }
    }
}
