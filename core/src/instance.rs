//! The `hello`'s `instance`: 16 random bytes, base64url, generated once per process of the addon
//! and reused on every reconnection of that process. The plugin uses it to count game clients
//! (two accounts running at once are two instances of one presence); it is not a secret and it
//! authenticates nothing, the token does that.
//!
//! The randomness comes from the standard library's `RandomState`, whose SipHash keys are drawn
//! from the operating system's random source (`ProcessPrng` on Windows) the first time a thread
//! asks for one. That is enough for an id that only has to differ between processes, and it
//! spares a dependency (`getrandom`) that nothing else in this workspace pulls in.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{encode_base64url, INSTANCE_BYTES};

/// A fresh instance id. Call it once, at load, and keep the value for the life of the process.
pub fn new_instance_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_nanos()).unwrap_or(0);
    let mut bytes = [0u8; INSTANCE_BYTES];
    for (half, chunk) in bytes.chunks_mut(8).enumerate() {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_usize(half);
        hasher.write_u128(nanos);
        hasher.write_u32(std::process::id());
        chunk.copy_from_slice(&hasher.finish().to_le_bytes());
    }
    encode_base64url(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::is_canonical_instance;

    #[test]
    fn is_what_the_plugin_accepts_as_an_instance() {
        for _ in 0..100 {
            let id = new_instance_id();
            assert!(is_canonical_instance(&id), "{id}");
        }
    }

    #[test]
    fn two_calls_differ() {
        assert_ne!(new_instance_id(), new_instance_id());
    }
}
