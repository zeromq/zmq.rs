use consts::ErrorCode;
//use std;
//use std::old_io::{IoError, IoErrorKind};


pub type ZmqResult<T> = Result<T, ZmqError>;

#[derive(Debug)]
pub struct ZmqError {
    pub code: ErrorCode,
    pub desc: &'static str,
    pub detail: Option<String>,
//    pub iokind: Option<IoErrorKind>,
}

impl ZmqError {
    pub fn new(code: ErrorCode, desc: &'static str) -> ZmqError {
        ZmqError {
            code: code,
            desc: desc,
            detail: None,
//            iokind: None,
        }
    }

//    pub fn from_io_error(e: IoError) -> ZmqError {
//        ZmqError {
//            code: match e.kind {
//                std::old_io::PermissionDenied => ErrorCode::EACCES,
//                std::old_io::ConnectionRefused => ErrorCode::ECONNREFUSED,
//                std::old_io::ConnectionReset => ErrorCode::ECONNRESET,
//                std::old_io::ConnectionAborted => ErrorCode::ECONNABORTED,
//                std::old_io::NotConnected => ErrorCode::ENOTCONN,
//                std::old_io::TimedOut => ErrorCode::ETIMEDOUT,
//
//                _ => ErrorCode::EIOERROR,
//            },
//            desc: e.desc,
//            detail: e.detail,
//            iokind: Some(e.kind),
//        }
//    }
}
