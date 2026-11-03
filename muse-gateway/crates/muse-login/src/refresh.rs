//! The pure poll decision of the login window (`LoginForm.RefreshStatus`).
//!
//! Why this lives in its own platform-independent module: `windows.rs` is
//! `#[cfg(windows)]` and never compiles on the Linux VM, so the branch
//! logic of `refresh_inner` — the part that decided the 3 Nov 2026
//! "silent return" bug — is decided here and unit-tested here.
//! `windows.rs` only *executes* the decision (set a status, inject the
//! check, save to the gateway).
//!
//! The decision mirrors `refresh_inner` exactly, in this order:
//!
//! 1. required cookies missing/empty → the user has not signed in yet;
//! 2. session already verified → save the cookies to the gateway
//!    (the page host is irrelevant here — verification already happened
//!    on `muse.ai`);
//! 3. inside the 15 s check throttle → do nothing, the status already
//!    shows the last state;
//! 4. page host is not `muse.ai` (e.g. a Meta/OAuth redirect has the
//!    cookies but the Muse page has not loaded back yet) → show the
//!    waiting-redirect status and inject nothing;
//! 5. otherwise → inject the `fetch('/api/session')` check.
//!
//! Step 4 is Perbaikan 1 (3 Nov 2026, approved ~11:31 WIB): the old code
//! returned silently here, so the window sat forever on "Waiting for you
//! to sign in" with no Retry — exactly Bree's 11:22 symptom (login not
//! saved, window closed by hand). The gate itself stays: the session
//! check is still only ever injected on `muse.ai`.

/// What one poll step of the login window should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDecision {
    /// Required cookies are missing or empty — the user has not signed
    /// in yet (or the session expired). Keep showing "Waiting for you
    /// to sign in".
    WaitForSignIn,
    /// A session check was injected less than the throttle interval ago.
    /// Do nothing; the status already shows the last state.
    Throttled,
    /// Cookies are complete but the page is not `muse.ai` (e.g. the
    /// sign-in is still on a Meta/OAuth redirect page). Show the
    /// waiting-redirect status (`LOGIN_WAITING_REDIRECT`) and inject
    /// nothing. The next poll re-decides, so as soon as the host is
    /// `muse.ai` again the check is injected as before.
    WaitingRedirect,
    /// Inject the `fetch('/api/session')` check into the `muse.ai` page.
    InjectCheck,
    /// Session already verified — save the cookies to the gateway.
    Save,
}

/// Decide one poll step.
///
/// * `cookies_complete` — `has_all_required` on the WebView cookies;
/// * `verified` — the `muse-session-check` postMessage already confirmed
///   the session (`session_verified`);
/// * `throttled` — `now < next_session_check`, i.e. a check was injected
///   less than 15 s ago;
/// * `host_is_muse` — the current page host is `muse.ai`.
pub fn decide_refresh(
    cookies_complete: bool,
    verified: bool,
    throttled: bool,
    host_is_muse: bool,
) -> RefreshDecision {
    if !cookies_complete {
        return RefreshDecision::WaitForSignIn;
    }
    if verified {
        return RefreshDecision::Save;
    }
    if throttled {
        return RefreshDecision::Throttled;
    }
    if !host_is_muse {
        return RefreshDecision::WaitingRedirect;
    }
    RefreshDecision::InjectCheck
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_cookies_wait_for_sign_in_whatever_else() {
        // Even "verified" cannot happen with incomplete cookies; the
        // cookie check wins so a stale verified flag can never trigger
        // a save of an empty session.
        assert_eq!(
            decide_refresh(false, false, false, true),
            RefreshDecision::WaitForSignIn
        );
        assert_eq!(
            decide_refresh(false, true, false, true),
            RefreshDecision::WaitForSignIn
        );
        assert_eq!(
            decide_refresh(false, false, false, false),
            RefreshDecision::WaitForSignIn
        );
    }

    #[test]
    fn complete_cookies_on_other_host_wait_for_redirect() {
        // The 3 Nov 2026 bug branch: cookies complete + page on an
        // OAuth redirect host must NOT silently return — it is a
        // visible waiting state, and it must NOT inject the check.
        assert_eq!(
            decide_refresh(true, false, false, false),
            RefreshDecision::WaitingRedirect
        );
    }

    #[test]
    fn complete_cookies_on_muse_ai_inject_the_check() {
        assert_eq!(
            decide_refresh(true, false, false, true),
            RefreshDecision::InjectCheck
        );
    }

    #[test]
    fn throttled_poll_injects_nothing_even_on_muse_ai() {
        assert_eq!(
            decide_refresh(true, false, true, true),
            RefreshDecision::Throttled
        );
        assert_eq!(
            decide_refresh(true, false, true, false),
            RefreshDecision::Throttled
        );
    }

    #[test]
    fn verified_session_saves_regardless_of_host() {
        // Verification already happened on muse.ai; the save must not
        // be blocked by a later navigation away from the page.
        assert_eq!(
            decide_refresh(true, true, false, true),
            RefreshDecision::Save
        );
        assert_eq!(
            decide_refresh(true, true, false, false),
            RefreshDecision::Save
        );
        assert_eq!(
            decide_refresh(true, true, true, true),
            RefreshDecision::Save
        );
    }
}
