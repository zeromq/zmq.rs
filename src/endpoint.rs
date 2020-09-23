use crate::error::ZmqError;
use lazy_static::lazy_static;
use regex::Regex;
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

// TODO: Figure out better error types for this module.

pub type Port = u16;

/// Represents a host address. Does not include the port, and may be either an
/// ip address or a domain name
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Host {
    /// An IPv4 address
    Ipv4(Ipv4Addr),
    /// An Ipv6 address
    Ipv6(Ipv6Addr),
    /// A domain name, such as `example.com` in `tcp://example.com:4567`.
    Domain(String),
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Host::Ipv4(addr) => write!(f, "{}", addr),
            Host::Ipv6(addr) => write!(f, "[{}]", addr),
            Host::Domain(name) => write!(f, "{}", name),
        }
    }
}

impl TryFrom<String> for Host {
    type Error = ZmqError;

    /// An Ipv6 address must be enclosed by `[` and `]`.
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.is_empty() {
            return Err(ZmqError::Other("Host string should not be empty"));
        }
        if let Ok(addr) = s.parse::<Ipv4Addr>() {
            return Ok(Host::Ipv4(addr));
        }
        if s.len() >= 4 {
            if let Ok(addr) = s[1..s.len() - 1].parse::<Ipv6Addr>() {
                return Ok(Host::Ipv6(addr));
            }
        }
        Ok(Host::Domain(s))
    }
}

impl FromStr for Host {
    type Err = ZmqError;

    /// Equivalent to [`Self::try_from()`]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.to_string();
        Self::try_from(s)
    }
}

/// The type of transport used by a given endpoint
#[derive(Debug, Clone, Hash, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Transport {
    /// TCP transport
    Tcp,
}

impl FromStr for Transport {
    type Err = ZmqError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let result = match s {
            "tcp" => Transport::Tcp,
            _ => return Err(ZmqError::Other("Unknown transport type")),
        };
        Ok(result)
    }
}
impl TryFrom<&str> for Transport {
    type Error = ZmqError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl fmt::Display for Transport {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        let s = match self {
            Transport::Tcp => "tcp",
        };
        write!(f, "{}", s)
    }
}

/// Represents a ZMQ Endpoint.
///
/// # Examples
/// ```
/// # use zeromq::{Endpoint, Host};
/// # fn main() -> Result<(), zeromq::ZmqError> {
/// assert_eq!(
///     "tcp://example.com:4567".parse::<Endpoint>()?,
///     Endpoint::Tcp(Host::Domain("example.com".to_string()), 4567)
/// );
/// # Ok(()) }
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub enum Endpoint {
    // TODO: Add endpoints for the other transport variants
    Tcp(Host, Port),
}

impl Endpoint {
    pub fn transport(&self) -> Transport {
        match self {
            Self::Tcp(_, _) => Transport::Tcp,
        }
    }
}

impl FromStr for Endpoint {
    type Err = ZmqError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        lazy_static! {
            static ref TRANSPORT_REGEX: Regex = Regex::new(r"^(.+)://(.+)$").unwrap();
            static ref HOST_PORT_REGEX: Regex = Regex::new(r"^(.+):(\d+)$").unwrap();
        }

        let caps = TRANSPORT_REGEX
            .captures(s)
            .ok_or(ZmqError::Other("Invalid Syntax"))?;
        let transport: &str = caps.get(1).unwrap().into();
        let transport: Transport = transport.parse()?;
        let address = caps.get(2).unwrap().as_str();

        fn extract_host_port(address: &str) -> Result<(Host, Port), ZmqError> {
            let caps = HOST_PORT_REGEX
                .captures(address)
                .ok_or(ZmqError::Other("Invalid Syntax"))?;
            let host = caps.get(1).unwrap().as_str();
            let port = caps.get(2).unwrap().as_str();
            let port: Port = port
                .parse()
                .map_err(|_| ZmqError::Other("Port must be a u16 but was out of range"))?;

            let host: Host = host.parse()?;
            Ok((host, port))
        }

        let endpoint = match transport {
            Transport::Tcp => {
                let (host, port) = extract_host_port(address)?;
                Endpoint::Tcp(host, port)
            }
        };

        Ok(endpoint)
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        match self {
            Endpoint::Tcp(host, port) => write!(f, "tcp://{}:{}", host, port),
        }
    }
}

// Trait aliases (https://github.com/rust-lang/rust/issues/41517) would make this unecessary
/// Any type that can be converted into an [`Endpoint`] should implement this
pub trait TryIntoEndpoint: Send {
    fn try_into(self) -> Result<Endpoint, ZmqError>;
}

impl<T> TryIntoEndpoint for T
where
    T: TryInto<Endpoint, Error = ZmqError> + Send,
{
    fn try_into(self) -> Result<Endpoint, ZmqError> {
        self.try_into()
    }
}

impl TryIntoEndpoint for &str {
    fn try_into(self) -> Result<Endpoint, ZmqError> {
        self.parse()
    }
}

impl TryIntoEndpoint for String {
    fn try_into(self) -> Result<Endpoint, ZmqError> {
        self.parse()
    }
}

impl TryIntoEndpoint for Endpoint {
    fn try_into(self) -> Result<Endpoint, ZmqError> {
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_static::lazy_static;

    lazy_static! {
        static ref PAIRS: Vec<(Endpoint, &'static str)> = vec![
            (
                Endpoint::Tcp(Host::Domain("www.example.com".to_string()), 1234),
                "tcp://www.example.com:1234",
            ),
            (
                Endpoint::Tcp(Host::Domain("*".to_string()), 2345),
                "tcp://*:2345",
            ),
            (
                Endpoint::Tcp(Host::Ipv4("127.0.0.1".parse().unwrap()), 8080),
                "tcp://127.0.0.1:8080",
            ),
            (
                Endpoint::Tcp(Host::Ipv6("::1".parse().unwrap()), 34567),
                "tcp://[::1]:34567",
            ),
        ];
    }

    #[test]
    fn test_endpoint_display() {
        for (e, s) in PAIRS.iter() {
            assert_eq!(&format!("{}", e), s);
        }
    }

    #[test]
    fn test_endpoint_parse() {
        for (e, s) in PAIRS.iter() {
            assert_eq!(&s.parse::<Endpoint>().unwrap(), e);
        }
    }
}
