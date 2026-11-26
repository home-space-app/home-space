//! 32-byte secrets that wipe themselves.

/// Declares a 32-byte secret newtype: zeroized on drop, no `Clone`, a `Debug` that hides the
/// bytes, and access to the bytes only inside the crate.
macro_rules! secret32 {
    ($(#[$meta:meta])* $vis:vis struct $name:ident;) => {
        $(#[$meta])*
        #[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
        $vis struct $name([u8; 32]);

        impl $name {
            /// Wraps raw bytes; the caller should wipe its own copy.
            #[allow(dead_code)]
            pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }

            #[allow(dead_code)]
            pub(crate) fn random() -> Self {
                let mut bytes = [0u8; 32];
                rand::fill(&mut bytes);
                Self(bytes)
            }

            pub(crate) fn expose(&self) -> &[u8; 32] {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(…)"))
            }
        }
    };
}

pub(crate) use secret32;
