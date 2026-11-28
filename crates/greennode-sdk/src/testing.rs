use crate::{ApiError, Error, ErrorClass, LoginError, error::SafeText};

#[cfg(feature = "test-support")]
pub fn allocation_probe(
    secrets: &[crate::Secret],
    trace: fn(u8),
    body: &[u8],
) -> (u8, std::time::Duration, std::time::Duration, usize) {
    let started = std::time::Instant::now();
    crate::wire::product::response::set_allocation_trace(trace);
    let mut credentials = crate::wire::product::evidence::SentCredentials::default();
    for secret in secrets {
        trace(6);
        let previous = credentials.clone();
        trace(7);
        credentials.record_bearer(secret);
        credentials.record_wire(&crate::Secret::new(format!(
            "Bearer {}",
            secret.expose_secret()
        )));
        if credentials.id_reflects("unrelated-probe") {
            return (3, started.elapsed(), std::time::Duration::ZERO, 0);
        }
        drop(previous);
    }
    let compile = started.elapsed();
    let started = std::time::Instant::now();
    trace(0);
    let evidence = crate::wire::product::evidence::Evidence::from_credentials(&credentials);
    let result = u8::from(!evidence.peer_body_screen(body).is_clean());
    (
        result,
        compile,
        started.elapsed(),
        credentials.pattern_peak_bytes().unwrap_or_default(),
    )
}

pub fn identity(class: ErrorClass) -> Error {
    Error::new(class)
}

pub fn api(operation: &'static str, status: Option<u16>, code: &str, message: &str) -> Error {
    Error::from_api(ApiError::new(
        operation,
        status,
        SafeText::new(code, &[]),
        SafeText::new(message, &[]),
        false,
    ))
}

pub fn login(reason: &'static str, status: Option<u16>) -> Error {
    Error::new(ErrorClass::Auth).with_login(LoginError::new(reason, status, false))
}

pub fn cause(error: Error, child: Error) -> Error {
    error.with_cause(child)
}

pub use crate::wire::test_server::{Response, Server};

macro_rules! products {
    ($( $variant:ident => $field:ident ),+ $(,)?) => {
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub enum Product { $( $variant ),+ }
        impl Product {
            pub(crate) fn override_slot(self, options: &mut crate::EndpointOverrides) -> &mut Option<String> {
                match self { $( Self::$variant => &mut options.$field ),+ }
            }
            pub(crate) fn endpoint(self, endpoints: &crate::endpoint::EndpointSet) -> Option<&str> {
                match self { $( Self::$variant => endpoints.$field.as_deref() ),+ }
            }
        }
    };
}
products! {
    Vserver => vserver, Vlb => vlb, Vnetwork => vnetwork,
    GlobalLoadBalancer => global_load_balancer, Dns => dns,
    ContainerRegistry => container_registry, Portal => portal, Billing => billing,
    Monitor => monitor, Iam => iam, Storage => storage, Cdn => cdn, CdnDocs => cdn_docs, Vks => vks,
    VserverBackup => vserver_backup, BackupCenter => backup_center, Database => database,
}

pub fn with_product_loopback(
    product: Product,
    options: crate::ConfigOptions,
    server: &Server,
) -> crate::ConfigOptions {
    options.with_product_loopback(product, server, false)
}

pub fn with_clock(
    options: crate::ConfigOptions,
    clock: std::sync::Arc<dyn crate::Clock>,
) -> crate::ConfigOptions {
    options.with_clock(clock)
}

#[cfg(any(test, feature = "test-support"))]
type VolumeConfirmationHook = Box<dyn Fn(&ResponseBodyClock) + Send + Sync>;

#[cfg(any(test, feature = "test-support"))]
pub struct ResponseBodyClock {
    clock: crate::TestClock,
    volume_confirmation: Option<VolumeConfirmationHook>,
    reading: tokio::sync::watch::Sender<usize>,
    sleep_started: tokio::sync::watch::Sender<std::collections::BTreeSet<std::time::Duration>>,
}

#[cfg(any(test, feature = "test-support"))]
impl crate::Clock for ResponseBodyClock {
    fn utc_now(&self) -> time::OffsetDateTime {
        self.clock.utc_now()
    }

    fn monotonic_now(&self) -> tokio::time::Instant {
        self.clock.monotonic_now()
    }

    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let delay = deadline.saturating_duration_since(self.clock.monotonic_now());
        self.sleep_started
            .send_if_modified(|started| started.insert(delay));
        self.clock.sleep_until(deadline)
    }
    fn response_body_started(&self) {
        self.reading.send_modify(|started| *started += 1);
    }

    fn volume_confirmation_read_completed(&self) {
        if let Some(hook) = &self.volume_confirmation {
            hook(self);
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ResponseBodyClock {
    pub fn new() -> Self {
        Self {
            clock: crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
            reading: tokio::sync::watch::channel(0).0,
            volume_confirmation: None,
            sleep_started: tokio::sync::watch::channel(std::collections::BTreeSet::new()).0,
        }
    }

    pub fn on_volume_confirmation(mut self, hook: impl Fn(&Self) + Send + Sync + 'static) -> Self {
        self.volume_confirmation = Some(Box::new(hook));
        self
    }

    pub async fn started(&self) {
        self.wait_for_body(1).await;
    }

    pub async fn wait_for_body(&self, count: usize) {
        self.reading
            .subscribe()
            .wait_for(|&started| started >= count)
            .await
            .unwrap();
    }

    pub async fn wait_for_one_second_retry(&self) {
        self.wait_for_sleep(std::time::Duration::from_secs(1)).await;
    }

    pub async fn wait_for_sleep(&self, duration: std::time::Duration) {
        self.sleep_started
            .subscribe()
            .wait_for(|started| started.contains(&duration))
            .await
            .unwrap();
    }

    pub fn advance(&self, duration: std::time::Duration) {
        self.clock.advance(duration);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for ResponseBodyClock {
    fn default() -> Self {
        Self::new()
    }
}

pub fn clock_advancing_on_sleep(
    clock: crate::TestClock,
    delay: std::time::Duration,
) -> impl crate::Clock {
    advancing_clock(clock, move |remaining| remaining == delay)
}

pub fn retry_clock() -> std::sync::Arc<dyn crate::Clock> {
    std::sync::Arc::new(advancing_clock(
        crate::TestClock::new(time::OffsetDateTime::UNIX_EPOCH),
        |remaining| !remaining.is_zero() && remaining <= std::time::Duration::from_secs(4),
    ))
}

fn advancing_clock(
    clock: crate::TestClock,
    should_advance: impl Fn(std::time::Duration) -> bool + Send + Sync,
) -> impl crate::Clock {
    struct Clock<F> {
        inner: crate::TestClock,
        should_advance: F,
    }

    impl<F: Fn(std::time::Duration) -> bool + Send + Sync> crate::Clock for Clock<F> {
        fn utc_now(&self) -> time::OffsetDateTime {
            self.inner.utc_now()
        }

        fn monotonic_now(&self) -> tokio::time::Instant {
            self.inner.monotonic_now()
        }

        fn sleep_until(
            &self,
            deadline: tokio::time::Instant,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
            let remaining = deadline.saturating_duration_since(self.monotonic_now());
            if (self.should_advance)(remaining) {
                Box::pin(async move { self.inner.advance(remaining) })
            } else {
                self.inner.sleep_until(deadline)
            }
        }
    }

    Clock {
        inner: clock,
        should_advance,
    }
}

pub fn with_write_loopback(
    product: Product,
    options: crate::ConfigOptions,
    server: &Server,
) -> crate::ConfigOptions {
    options.with_product_loopback(product, server, true)
}

#[derive(Clone)]
pub(crate) struct ProductFixture {
    pub(crate) product: Product,
    pub(crate) trust: crate::wire::connection::FixtureTrust,
    pub(crate) write: bool,
}

pub fn with_billing_loopback(
    options: crate::ConfigOptions,
    server: &Server,
) -> crate::ConfigOptions {
    with_product_loopback(Product::Billing, options, server)
}
