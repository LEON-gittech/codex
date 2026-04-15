use http::HeaderMap;

/// Adds authentication headers to API requests.
///
/// Implementations should be cheap and non-blocking; any asynchronous
/// refresh or I/O should be handled by higher layers before requests
/// reach this interface.
pub trait AuthProvider: Send + Sync {
    fn add_auth_headers(&self, headers: &mut HeaderMap);

    fn auth_header_attached(&self) -> bool {
        false
    }

    fn auth_header_name(&self) -> Option<&'static str> {
        None
    }
}
