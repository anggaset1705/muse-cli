//! Driver HTTP — port `HttpDriver` (`Drivers.cs`).
//!
//! Direct HTTP protocol driver — dicadangkan di Python juga (501 untuk
//! chat). Renew sesi sudah jalan via HTTP murni dan di-port ke sini
//! (`.NET: Upstream.RenewSession`).

use async_trait::async_trait;

use crate::driver::MuseDriver;
use crate::error::MuseError;
use crate::types::{Account, ChatRequest, DriverCapabilities, SessionInfo};
use crate::upstream::Upstream;
use crate::ChatStream;

/// Port `HttpDriver` (`Drivers.cs`).
pub struct HttpDriver;

impl HttpDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HttpDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MuseDriver for HttpDriver {
    fn name(&self) -> &'static str {
        "http"
    }

    fn requires_account(&self) -> bool {
        true
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            renew_session: true,
            ..DriverCapabilities::NONE
        }
    }

    fn chat_stream(&self, _account: Account, _req: ChatRequest) -> ChatStream {
        // .NET: throw MuseException.NotImplemented(
        //   "http driver chat is not implemented yet; use driver=browser")
        // Stream yang langsung error — pemanggil mendapat Err sebagai item
        // pertama (bukan panic).
        Box::pin(futures_util::stream::once(async {
            Err(MuseError::not_implemented(
                "http driver chat is not implemented yet; use driver=browser",
            ))
        }))
    }

    async fn renew_session(&self, account: Account) -> Result<SessionInfo, MuseError> {
        // .NET: Upstream.RenewSession(account.Cookies) — default
        // confirmSeconds=20, pollSeconds=2.
        Upstream::renew_session(&account.cookies, 20.0, 2.0).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::DriverFactory;
    use futures_util::StreamExt;

    #[test]
    fn http_chat_is_not_implemented_with_dotnet_message() {
        let driver = HttpDriver::new();
        assert_eq!(driver.name(), "http");
        assert!(driver.requires_account());
        assert!(driver.capabilities().renew_session);
        assert!(!driver.capabilities().chat);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = rt.block_on(async {
            let mut s = driver.chat_stream(Account::new("acc_x"), ChatRequest::new("hi", "muse-chat"));
            s.next().await.unwrap().unwrap_err()
        });
        assert_eq!(err.status, 501);
        assert_eq!(
            err.message,
            "http driver chat is not implemented yet; use driver=browser"
        );
    }

    #[test]
    fn factory_unknown_driver_errors_like_dotnet() {
        let err = match DriverFactory::create_by_name("selenium") {
            Ok(_) => panic!("unknown driver should fail"),
            Err(e) => e,
        };
        assert_eq!(err.message, "unknown driver 'selenium'");
    }

    #[test]
    fn factory_creates_all_three_drivers() {
        for (name, needs_account) in [("mock", false), ("browser", true), ("http", true)] {
            let d = DriverFactory::create_by_name(name).expect(name);
            assert_eq!(d.name(), name);
            assert_eq!(d.requires_account(), needs_account);
        }
    }
}
