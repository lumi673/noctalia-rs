/// Opaque handle for a process started by [`Context::run`](crate::context::Context::run).
#[derive(Clone, Debug)]
pub struct RunHandle {
    started: bool,
}

impl RunHandle {
    pub(crate) const fn new(started: bool) -> Self {
        Self { started }
    }

    /// Stops the started work.
    ///
    /// This is a no-op in the Rust library because auto/off cannot cancel host work under
    /// API 57. Strict emit suppresses this handle's callback; it does not cancel host work.
    pub const fn stop(&self) {}

    /// Whether the host accepted the start request.
    #[inline]
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }
}

/// Opaque handle for a request started by [`Context::http`](crate::context::Context::http).
#[derive(Clone, Debug)]
pub struct HttpHandle {
    started: bool,
}

impl HttpHandle {
    pub(crate) const fn new(started: bool) -> Self {
        Self { started }
    }

    /// Stops the started work.
    ///
    /// This is a no-op in the Rust library because auto/off cannot cancel host work under
    /// API 57. Strict emit suppresses this handle's callback; it does not cancel host work.
    pub const fn stop(&self) {}

    /// Whether the host accepted the start request.
    #[inline]
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }
}

/// Opaque handle for a stream started by [`Context::http_stream`](crate::context::Context::http_stream).
#[derive(Clone, Debug)]
pub struct HttpStreamHandle {
    token: Option<String>,
}

impl HttpStreamHandle {
    pub(crate) const fn new(token: Option<String>) -> Self {
        Self { token }
    }

    /// Stops the started work.
    ///
    /// This is a no-op in the Rust library because auto/off cannot cancel host work under
    /// API 57. Strict emit suppresses this handle's callback and forwards stop to the
    /// wrapped stream handle when the host provides one; it does not promise cancellation.
    pub const fn stop(&self) {}

    /// Whether the host returned a stream token.
    #[inline]
    #[must_use]
    pub const fn started(&self) -> bool {
        self.token.is_some()
    }
}

/// Opaque handle for a watch registered by [`Context::state_watch`](crate::context::Context::state_watch).
#[derive(Clone, Debug)]
pub struct StateWatchHandle {
    key: String,
    callback_id: String,
}

impl StateWatchHandle {
    pub(crate) const fn new(key: String, callback_id: String) -> Self {
        Self { key, callback_id }
    }

    /// Stops the started work.
    ///
    /// This is a no-op in the Rust library because auto/off cannot cancel host work under
    /// API 57. Strict emit suppresses this handle's callback; it does not cancel host work.
    pub const fn stop(&self) {}

    /// Whether the watch was registered with both a key and a callback.
    #[inline]
    #[must_use]
    pub const fn started(&self) -> bool {
        !self.key.is_empty() && !self.callback_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_and_http_handles_report_acceptance() {
        let run = RunHandle::new(true);
        let http = HttpHandle::new(false);

        assert!(run.started());
        assert!(!http.started());

        run.stop();
        http.stop();

        assert!(run.started());
        assert!(!http.started());
    }

    #[test]
    fn test_stream_started_depends_on_the_host_token() {
        assert!(HttpStreamHandle::new(Some("stream".to_owned())).started());
        assert!(!HttpStreamHandle::new(None).started());
    }

    #[test]
    fn test_watch_started_depends_on_the_registration_arguments() {
        let watch =
            StateWatchHandle::new("key".to_owned(), "callback".to_owned());
        let empty = StateWatchHandle::new(String::new(), "callback".to_owned());

        assert!(watch.started());
        assert!(!empty.started());

        watch.stop();
        assert!(watch.started());
    }
}
