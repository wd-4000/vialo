use lettre::message::header::{Header, HeaderName, HeaderValue};

macro_rules! text_header {
    ($(#[$attr:meta])* Header($type_name: ident, $header_name: expr )) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $type_name(pub String);

        impl Header for $type_name {
            fn name() -> HeaderName {
                HeaderName::new_from_ascii_str($header_name)
            }

            fn parse(s: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                Ok(Self(s.into()))
            }

            fn display(&self) -> HeaderValue {
             HeaderValue::new(Self::name(),   self.0.clone())
            }
        }

        impl From<String> for $type_name {
            #[inline]
            fn from(text: String) -> Self {
                Self(text)
            }
        }

        impl AsRef<str> for $type_name {
            #[inline]
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

text_header!(Header(ListUnsubscribe, "List-Unsubscribe"));
text_header!(Header(ListUnsubscribePost, "List-Unsubscribe-Post"));
