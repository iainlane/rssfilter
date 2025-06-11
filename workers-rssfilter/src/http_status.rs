use http::StatusCode;
use std::sync::LazyLock;

macro_rules! status_code {
  ($($name:ident => $variant:ident),* $(,)?) => {
    $(
      pub static $name: LazyLock<u16> = LazyLock::new(|| StatusCode::$variant.as_u16());
    )*
  };
}

status_code! {
  BAD_GATEWAY => BAD_GATEWAY,
  BAD_REQUEST => BAD_REQUEST,
  INTERNAL_SERVER_ERROR => INTERNAL_SERVER_ERROR,
  NOT_FOUND => NOT_FOUND,
  METHOD_NOT_ALLOWED => METHOD_NOT_ALLOWED,
  PAYLOAD_TOO_LARGE => PAYLOAD_TOO_LARGE,
  UNSUPPORTED_MEDIA_TYPE => UNSUPPORTED_MEDIA_TYPE,
}
