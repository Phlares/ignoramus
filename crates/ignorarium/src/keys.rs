//! The box signing key: 32 raw bytes in `keys/box.ed25519`.
use crate::files::{WriteOnceError, write_once};
use crate::home::Home;
use ed25519_dalek::SigningKey;
use std::{fs, io};

#[derive(Debug)]
pub enum KeyError {
    /// The key file holds this many bytes instead of 32.
    BadLength(usize),
    /// The key path is a symbolic link.
    Link,
    Io(io::Error),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::BadLength(n) => write!(f, "key file has {n} bytes, expected 32"),
            KeyError::Link => write!(f, "key path is a symbolic link"),
            KeyError::Io(e) => write!(f, "I/O failed: {e}"),
        }
    }
}

impl std::error::Error for KeyError {}

impl From<io::Error> for KeyError {
    fn from(e: io::Error) -> Self {
        KeyError::Io(e)
    }
}

/// Load the box key, or create it. A key file of the wrong length is an
/// error and is never replaced.
pub fn load_or_create(home: &Home) -> Result<SigningKey, KeyError> {
    let path = home.root.join("keys").join("box.ed25519");
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
    match write_once(&path, &seed, 0o600) {
        Ok(_) | Err(WriteOnceError::Differs) => {}
        Err(WriteOnceError::Link) => return Err(KeyError::Link),
        Err(WriteOnceError::Io(e)) => return Err(e.into()),
    }
    let bytes = fs::read(&path)?;
    let seed: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| KeyError::BadLength(bytes.len()))?;
    Ok(SigningKey::from_bytes(&seed))
}
