use crate::{ApiError, Error, ErrorClass, LoginError, error::SafeText};

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
