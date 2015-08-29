use consts::ErrorCode;
use std::error;
use std::io::{Error, ErrorKind};


pub type ZmqResult<T> = Result<T, ZmqError>;

#[derive(Debug)]
pub struct ZmqError {
	code: ErrorCode,
    repr: Repr,
}

#[derive(Debug)]
enum Repr {
    Io(Box<Error>),
    Custom(&'static str),
}

impl ZmqError {
    pub fn new(code: ErrorCode, desc: &'static str) -> ZmqError {
        ZmqError {
            code: code,
            repr: Repr::Custom(desc),
        }
    }

    pub fn from_io_error(e: Error) -> ZmqError {
        ZmqError {
            code: match e.kind() {
                ErrorKind::PermissionDenied => ErrorCode::EACCES,
                ErrorKind::ConnectionRefused => ErrorCode::ECONNREFUSED,
                ErrorKind::ConnectionReset => ErrorCode::ECONNRESET,
                ErrorKind::ConnectionAborted => ErrorCode::ECONNABORTED,
                ErrorKind::NotConnected => ErrorCode::ENOTCONN,
                ErrorKind::TimedOut => ErrorCode::ETIMEDOUT,

                _ => ErrorCode::EIOERROR,
            },
            repr: Repr::Io(Box::new(e)),
        }
    }
}
