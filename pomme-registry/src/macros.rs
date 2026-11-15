//! The macros the generated entry lists are written with.

/// A builtin registry: an enum whose variants are its entries in protocol-id
/// order, each with its path under the `minecraft` namespace.
macro_rules! registry {
    ($name:ident { $($variant:ident => $path:literal,)* }) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(u32)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            /// Every entry, in protocol-id order.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];
            const NAMES: &'static [&'static str] = &[$(concat!("minecraft:", $path),)*];

            /// The entry's identifier, like `minecraft:stone`.
            pub fn to_str(&self) -> &'static str {
                Self::NAMES[*self as usize]
            }
        }

        impl $crate::Registry for $name {
            fn from_u32(id: u32) -> Option<Self> {
                Self::ALL.get(id as usize).copied()
            }

            fn to_u32(&self) -> u32 {
                *self as u32
            }
        }

        impl TryFrom<u32> for $name {
            type Error = ();

            fn try_from(id: u32) -> Result<Self, ()> {
                $crate::Registry::from_u32(id).ok_or(())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.to_str())
            }
        }

        impl TryFrom<&$crate::Identifier> for $name {
            type Error = ();

            fn try_from(ident: &$crate::Identifier) -> Result<Self, ()> {
                if ident.namespace() != $crate::identifier::DEFAULT_NAMESPACE {
                    return Err(());
                }
                match ident.path() {
                    $($path => Ok(Self::$variant),)*
                    _ => Err(()),
                }
            }
        }

        impl From<$name> for $crate::Identifier {
            fn from(entry: $name) -> Self {
                Self::new(entry.to_str())
            }
        }

        /// From an identifier with or without its namespace.
        impl std::str::FromStr for $name {
            type Err = ();

            fn from_str(s: &str) -> Result<Self, ()> {
                Self::try_from(&$crate::Identifier::new(s))
            }
        }

        #[cfg(feature = "azalea")]
        $crate::azalea::builtin_registry!($name);
    };
}

/// A data registry: an opaque id, and a key enum of the entries the game's
/// data pack has, with `Other` for any a server adds.
macro_rules! data_registry {
    ($registry:ident, $name:literal, $key:ident { $($variant:ident => $path:literal,)* }) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $registry {
            id: u32,
        }

        impl $crate::DataRegistry for $registry {
            const NAME: &'static str = $name;
            type Key = $key;

            fn protocol_id(&self) -> u32 {
                self.id
            }

            fn new_raw(id: u32) -> Self {
                Self { id }
            }
        }

        #[derive(Clone, Debug, Eq, Hash, PartialEq)]
        pub enum $key<Other = $crate::Identifier> {
            $($variant,)*
            Other(Other),
        }

        impl $key {
            /// Every entry the game's data pack has.
            pub const ALL: &'static [Self] = &[$(Self::$variant,)*];
        }

        impl<'a> From<&'a $crate::Identifier> for $key<&'a $crate::Identifier> {
            fn from(ident: &'a $crate::Identifier) -> Self {
                if ident.namespace() != $crate::identifier::DEFAULT_NAMESPACE {
                    return Self::Other(ident);
                }
                match ident.path() {
                    $($path => Self::$variant,)*
                    _ => Self::Other(ident),
                }
            }
        }

        impl $crate::DataRegistryKey for $key {
            type Borrow<'a> = $key<&'a $crate::Identifier>;

            fn from_ident(ident: $crate::Identifier) -> Self {
                $crate::DataRegistryKeyRef::to_owned(<$key<&$crate::Identifier>>::from(&ident))
            }

            fn into_ident(self) -> $crate::Identifier {
                match self {
                    $(Self::$variant => $crate::Identifier::new($path),)*
                    Self::Other(ident) => ident,
                }
            }
        }

        impl<'a> $crate::DataRegistryKeyRef<'a> for $key<&'a $crate::Identifier> {
            type Owned = $key;

            fn to_owned(self) -> $key {
                match self {
                    $(Self::$variant => $key::$variant,)*
                    Self::Other(ident) => $key::Other(ident.clone()),
                }
            }

            fn from_ident(ident: &'a $crate::Identifier) -> Self {
                Self::from(ident)
            }

            fn into_ident(self) -> $crate::Identifier {
                $crate::DataRegistryKey::into_ident(self.to_owned())
            }
        }

        impl From<$crate::Identifier> for $key {
            fn from(ident: $crate::Identifier) -> Self {
                $crate::DataRegistryKey::from_ident(ident)
            }
        }

        impl From<$key> for $crate::Identifier {
            fn from(key: $key) -> Self {
                $crate::DataRegistryKey::into_ident(key)
            }
        }

        impl From<$key<&$crate::Identifier>> for $crate::Identifier {
            fn from(key: $key<&$crate::Identifier>) -> Self {
                $crate::DataRegistryKeyRef::into_ident(key)
            }
        }

        #[cfg(feature = "azalea")]
        $crate::azalea::data_registry!($registry, $name, $key);
    };
}
