//! Newtypes for Scaleway lifecycle values to avoid stringly-typed code.

use std::ops::Deref;

macro_rules! newtype {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub(crate) struct $name(String);

        impl $name {
            #[expect(dead_code, reason = "builder retained for future ergonomics")]
            pub(crate) fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub(crate) const fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &Self::Target {
                self.as_str()
            }
        }
    };
}

newtype!(InstanceId);
newtype!(Zone);
newtype!(InstanceState);
newtype!(Action);
