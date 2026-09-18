use rmcp::ErrorData;
use tracing::error;

////////////////////////////////////////////////////////////////////////////////
pub type McpResult<T> = Result<T, ErrorData>;

////////////////////////////////////////////////////////////////////////////////
pub fn mcp_internal_error(message: impl Into<String>) -> ErrorData {
    let message = message.into();
    error!("{message}");
    ErrorData::internal_error(message, None)
}

////////////////////////////////////////////////////////////////////////////////
#[macro_export]
macro_rules! mcp_internal_error {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        $crate::helpers::errors::mcp_internal_error(message)
    }}
}
