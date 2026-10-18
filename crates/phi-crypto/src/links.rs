use crate::{CryptoError, GrantSecret};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use phi_proto::SpaceId;
use std::net::Ipv6Addr;
use zeroize::Zeroizing;

#[derive(Debug)]
pub struct GrantLink {
    tls: bool,
    authority: String,
    space_id: SpaceId,
    secret: GrantSecret,
}
impl GrantLink {
    pub fn new(
        tls: bool,
        authority: String,
        space_id: SpaceId,
        secret: GrantSecret,
    ) -> Result<Self, CryptoError> {
        validate_authority(&authority)?;
        Ok(Self {
            tls,
            authority,
            space_id,
            secret,
        })
    }
    pub fn parse(link: &str) -> Result<Self, CryptoError> {
        let (tls, remainder) = if let Some(rest) = link.strip_prefix("phis://") {
            (true, rest)
        } else if let Some(rest) = link.strip_prefix("phi://") {
            (false, rest)
        } else {
            return Err(CryptoError::Link("expected phi:// or phis://"));
        };
        let (location, secret) = remainder
            .split_once('#')
            .ok_or(CryptoError::Link("missing secret fragment"))?;
        let (authority, space) = location
            .split_once('/')
            .ok_or(CryptoError::Link("missing space id"))?;
        if secret.len() != 43 {
            return Err(CryptoError::Link(
                "secret must encode 32 bytes without padding",
            ));
        }
        let bytes = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(secret)
                .map_err(|_| CryptoError::Link("invalid secret encoding"))?,
        );
        if bytes.len() != 32 {
            return Err(CryptoError::Link("secret must be 32 bytes"));
        }
        let mut secret_bytes = Zeroizing::new([0; 32]);
        secret_bytes.copy_from_slice(&bytes);
        Self::new(
            tls,
            authority.to_owned(),
            space.parse()?,
            GrantSecret::from_bytes(*secret_bytes),
        )
    }
    pub fn encode(&self) -> Zeroizing<String> {
        let secret = Zeroizing::new(URL_SAFE_NO_PAD.encode(self.secret.expose()));
        Zeroizing::new(format!(
            "{}://{}/{}#{}",
            if self.tls { "phis" } else { "phi" },
            self.authority,
            self.space_id,
            secret.as_str()
        ))
    }
    pub fn server_url(&self) -> String {
        format!(
            "{}://{}",
            if self.tls { "https" } else { "http" },
            self.authority
        )
    }
    pub fn space_id(&self) -> SpaceId {
        self.space_id
    }
    pub fn secret(&self) -> &GrantSecret {
        &self.secret
    }
}
fn validate_authority(authority: &str) -> Result<(), CryptoError> {
    if authority.is_empty() || authority.len() > 260 || !authority.is_ascii() {
        return Err(CryptoError::Link("invalid server authority"));
    }
    let port = if let Some(rest) = authority.strip_prefix('[') {
        let (address, tail) = rest
            .split_once(']')
            .ok_or(CryptoError::Link("invalid IPv6 authority"))?;
        address
            .parse::<Ipv6Addr>()
            .map_err(|_| CryptoError::Link("invalid IPv6 address"))?;
        if tail.is_empty() {
            None
        } else {
            Some(
                tail.strip_prefix(':')
                    .ok_or(CryptoError::Link("invalid port"))?,
            )
        }
    } else {
        let (host, port) = if let Some((host, port)) = authority.split_once(':') {
            (host, Some(port))
        } else {
            (authority, None)
        };
        if host.len() > 253
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(CryptoError::Link("invalid host"));
        }
        port
    };
    if let Some(port) = port {
        if !port.bytes().all(|b| b.is_ascii_digit())
            || port.parse::<u16>().ok().is_none_or(|n| n == 0)
        {
            return Err(CryptoError::Link("invalid port"));
        }
    }
    Ok(())
}
