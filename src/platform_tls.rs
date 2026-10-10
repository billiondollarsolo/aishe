//! Preserve ureq's transport while activating delayed macOS frameworks on use.

/// All configuration, pooling, resolution and proxy behavior stays with ureq.
pub(crate) fn agent(config: ureq::config::Config) -> ureq::Agent {
    #[cfg(all(target_os = "macos", aishe_delayed_frameworks))]
    {
        ureq::Agent::with_parts(
            config,
            macos::ActivatingConnector::default(),
            ureq::unversioned::resolver::DefaultResolver::default(),
        )
    }
    #[cfg(not(all(target_os = "macos", aishe_delayed_frameworks)))]
    {
        config.into()
    }
}

pub(crate) fn default_agent() -> ureq::Agent {
    agent(ureq::config::Config::default())
}

#[cfg(all(target_os = "macos", aishe_delayed_frameworks))]
mod macos {
    use std::sync::OnceLock;
    use ureq::unversioned::transport::{ConnectionDetails, Connector, DefaultConnector, Transport};

    static ACTIVATED: OnceLock<Result<(), String>> = OnceLock::new();
    #[cfg(test)]
    pub(super) static ACTIVATION_ATTEMPTS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    #[derive(Debug, Default)]
    pub(super) struct ActivatingConnector {
        inner: DefaultConnector,
    }

    impl Connector<()> for ActivatingConnector {
        type Out = Box<dyn Transport>;

        fn connect(
            &self,
            details: &ConnectionDetails,
            chained: Option<()>,
        ) -> Result<Option<Self::Out>, ureq::Error> {
            if details.needs_tls() {
                activate().map_err(ureq::Error::Io)?;
            }
            // Activation finishes before delegating. HTTPS CONNECT proxies
            // recursively invoke this wrapper, so never hold an initializer
            // lock while invoking the default connector.
            self.inner.connect(details, chained)
        }
    }

    fn activate() -> std::io::Result<()> {
        ACTIVATED
            .get_or_init(|| {
                #[cfg(test)]
                ACTIVATION_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                for path in [
                    c"/System/Library/Frameworks/CoreFoundation.framework/Versions/A/CoreFoundation",
                    c"/System/Library/Frameworks/Security.framework/Versions/A/Security",
                ] {
                    // SAFETY: these are absolute, NUL-terminated system paths.
                    // dlopen runs each framework's initializers before returning.
                    // Keep its reference for the process lifetime: existing TLS
                    // objects and imported symbols must never outlive a handle.
                    let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
                    if handle.is_null() {
                        return Err("could not initialize macOS TLS frameworks".to_string());
                    }
                }
                Ok(())
            })
            .as_ref()
            .map_err(|message| std::io::Error::other(message.clone()))
            .copied()
    }
}

#[cfg(test)]
mod certificate_rejection {
    use std::io::{Error, ErrorKind};

    pub(super) fn is_certificate_rejection(error: &ureq::Error) -> bool {
        let tls_error = match error {
            ureq::Error::Rustls(error) => Some(error),
            // rustls complete_io stores its typed handshake failure in an
            // InvalidData IO error; ureq's stream transport preserves it.
            ureq::Error::Io(error) if error.kind() == ErrorKind::InvalidData => error
                .get_ref()
                .and_then(|error| error.downcast_ref::<rustls::Error>()),
            _ => None,
        };
        matches!(tls_error, Some(rustls::Error::InvalidCertificate(_)))
    }

    fn invalid_certificate() -> rustls::Error {
        rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer)
    }

    #[test]
    fn accepts_direct_and_io_wrapped_invalid_certificate() {
        assert!(is_certificate_rejection(&ureq::Error::Rustls(
            invalid_certificate()
        )));
        assert!(is_certificate_rejection(&ureq::Error::Io(Error::new(
            ErrorKind::InvalidData,
            invalid_certificate()
        ))));
    }

    #[test]
    fn rejects_network_failures_and_certificate_text() {
        for error in [
            ureq::Error::Timeout(ureq::Timeout::Global),
            ureq::Error::Io(Error::new(ErrorKind::TimedOut, "certificate timeout")),
            ureq::Error::Io(Error::new(ErrorKind::ConnectionReset, "certificate reset")),
            ureq::Error::Io(Error::new(
                ErrorKind::InvalidData,
                "invalid peer certificate: certificate is not trusted",
            )),
            ureq::Error::Tls("invalid peer certificate"),
        ] {
            assert!(!is_certificate_rejection(&error), "accepted {error}");
        }
    }

    #[test]
    fn rejects_noncertificate_tls_errors_and_wrong_io_kind() {
        for error in [
            ureq::Error::Rustls(rustls::Error::General("certificate failure".to_string())),
            ureq::Error::Io(Error::new(
                ErrorKind::InvalidData,
                rustls::Error::General("certificate failure".to_string()),
            )),
            ureq::Error::Io(Error::other(invalid_certificate())),
        ] {
            assert!(!is_certificate_rejection(&error), "accepted {error}");
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    fn assert_certificate_rejected(error: ureq::Error) {
        assert!(
            super::certificate_rejection::is_certificate_rejection(&error),
            "expected typed invalid-certificate rejection, got {error}"
        );
    }

    fn trusted_request(agent: &ureq::Agent, url: &str) {
        let mut response = agent.get(url).call().expect("trusted HTTPS");
        assert_eq!(response.status().as_u16(), 200);
        let body = response.body_mut().read_to_string().expect("trusted body");
        assert!(
            body.contains("Example Domain"),
            "expected actual public fixture body"
        );
    }

    #[test]
    #[ignore = "requires the bounded disposable native TLS fixture"]
    fn native_first_concurrent_https_and_untrusted_rejection() {
        println!(
            "native startup link record: {}",
            option_env!("AISHE_STARTUP_LINK_DIAGNOSTIC").unwrap_or("unavailable")
        );
        let trusted =
            std::env::var("AISHE_NATIVE_TLS_VALID_URL").expect("trusted HTTPS fixture URL");
        let untrusted =
            std::env::var("AISHE_NATIVE_TLS_UNTRUSTED_URL").expect("untrusted HTTPS fixture URL");
        assert!(trusted.starts_with("https://"));
        assert!(untrusted.starts_with("https://127.0.0.1:"));
        #[cfg(aishe_delayed_frameworks)]
        assert_eq!(
            super::macos::ACTIVATION_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );

        // The same production factory and cloned pool are used concurrently on
        // the first HTTPS requests. No keychain or trust-store mutation occurs.
        let agent = crate::providers::external_http_agent(
            Duration::from_secs(10),
            Some(Duration::from_secs(25)),
            Some(Duration::from_secs(10)),
            Some(Duration::from_secs(10)),
        );
        let barrier = Arc::new(Barrier::new(4));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let agent = agent.clone();
                let trusted = trusted.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    trusted_request(&agent, &trusted);
                })
            })
            .collect();
        for thread in threads {
            thread.join().expect("HTTPS request thread");
        }
        trusted_request(&agent, &trusted);
        let redirect =
            std::env::var("AISHE_NATIVE_TLS_REDIRECT_URL").expect("HTTP redirect fixture");
        trusted_request(&agent, &redirect);
        let error = agent
            .get(&untrusted)
            .call()
            .expect_err("self-signed certificate must be rejected");
        assert_certificate_rejected(error);
        #[cfg(aishe_delayed_frameworks)]
        assert_eq!(
            super::macos::ACTIVATION_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        println!("native TLS: concurrent first use, later pool use, HTTP redirect and untrusted certificate rejection passed");
    }

    #[test]
    #[ignore = "requires the bounded disposable native TLS fixture"]
    fn native_https_proxy_activates_for_http_target() {
        println!(
            "native startup link record: {}",
            option_env!("AISHE_STARTUP_LINK_DIAGNOSTIC").unwrap_or("unavailable")
        );
        let proxy = std::env::var("AISHE_NATIVE_TLS_UNTRUSTED_URL").expect("HTTPS proxy fixture");
        #[cfg(aishe_delayed_frameworks)]
        assert_eq!(
            super::macos::ACTIVATION_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let config = ureq::config::Config::builder()
            .proxy(Some(ureq::Proxy::new(&proxy).expect("HTTPS proxy URL")))
            .timeout_global(Some(Duration::from_secs(10)))
            .tls_config(
                ureq::tls::TlsConfig::builder()
                    .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                    .build(),
            )
            .build();
        // A documentation IP avoids DNS and is never contacted: the local
        // HTTPS proxy certificate must be rejected before CONNECT is sent.
        let error = super::agent(config)
            .get("http://192.0.2.1:1/")
            .call()
            .expect_err("untrusted HTTPS proxy certificate must be rejected");
        assert_certificate_rejected(error);
        #[cfg(aishe_delayed_frameworks)]
        assert_eq!(
            super::macos::ACTIVATION_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        println!("native TLS: HTTPS proxy first use for an HTTP target rejected its untrusted certificate");
    }
}
