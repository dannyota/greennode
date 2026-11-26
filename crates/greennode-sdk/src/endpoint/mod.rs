#![allow(dead_code)]

use url::Url;

pub(crate) struct Base {
    url: Url,
    serialized: String,
}

impl std::ops::Deref for Base {
    type Target = str;

    fn deref(&self) -> &str {
        &self.serialized
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BaseError {
    InvalidUrl,
    InvalidPort,
}

#[derive(Default)]
pub(crate) struct EndpointOverrides {
    pub(crate) vserver: Option<String>,
    pub(crate) vlb: Option<String>,
    pub(crate) vnetwork: Option<String>,
    pub(crate) global_load_balancer: Option<String>,
    pub(crate) dns: Option<String>,
    pub(crate) container_registry: Option<String>,
    pub(crate) portal: Option<String>,
    pub(crate) signin: Option<String>,
    pub(crate) dashboard: Option<String>,
    pub(crate) token: Option<String>,
    pub(crate) billing: Option<String>,
    pub(crate) cdn_docs: Option<String>,
    pub(crate) monitor: Option<String>,
    pub(crate) iam: Option<String>,
    pub(crate) storage: Option<String>,
    pub(crate) cdn: Option<String>,
    pub(crate) vks: Option<String>,
    pub(crate) vserver_backup: Option<String>,
    pub(crate) backup_center: Option<String>,
    pub(crate) database: Option<String>,
}

pub(crate) struct EndpointSet {
    pub(crate) vserver: Option<Base>,
    pub(crate) vlb: Option<Base>,
    pub(crate) vnetwork: Option<Base>,
    pub(crate) global_load_balancer: Option<Base>,
    pub(crate) dns: Option<Base>,
    pub(crate) container_registry: Option<Base>,
    pub(crate) portal: Option<Base>,
    pub(crate) signin: Option<Base>,
    pub(crate) dashboard: Option<Base>,
    pub(crate) token: Option<Base>,
    pub(crate) billing: Option<Base>,
    pub(crate) cdn_docs: Option<Base>,
    pub(crate) monitor: Option<Base>,
    pub(crate) iam: Option<Base>,
    pub(crate) storage: Option<Base>,
    pub(crate) cdn: Option<Base>,
    pub(crate) vks: Option<Base>,
    pub(crate) vserver_backup: Option<Base>,
    pub(crate) backup_center: Option<Base>,
    pub(crate) database: Option<Base>,
}

pub(crate) fn defaults(region: &str, overrides: EndpointOverrides) -> Result<EndpointSet, ()> {
    if !valid_region(region) {
        return Err(());
    }
    let console = format!("https://{region}.console.greennode.ai/vserver/");
    let dashboard = overrides
        .dashboard
        .clone()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "https://dashboard.console.greennode.ai/".into());
    let token = format!(
        "{}/accounts-api/v1/auth/token",
        dashboard.trim_end_matches('/')
    );
    let mut set = EndpointOverrides {
        vserver: Some(format!("{console}iam-vserver-gateway/")),
        vlb: Some(format!("{console}iam-vlb-gateway/")),
        vnetwork: Some(format!("{console}vnetwork-gateway/")),
        global_load_balancer: Some("https://glb.console.greennode.ai/glb-controller/".into()),
        dns: Some("https://vdns.console.greennode.ai/vdns-api/".into()),
        container_registry: Some("https://vcr.console.greennode.ai/vcr-api/".into()),
        portal: Some(format!("{console}iam-billing-gateway/")),
        signin: Some("https://signin.greennode.ai".into()),
        dashboard: Some(dashboard.clone()),
        token: Some(token),
        billing: Some("https://dashboard.console.greennode.ai/".into()),
        cdn_docs: Some("https://docs.greennode.ai/faq/vcdn".into()),
        monitor: Some("https://vmonitor.console.greennode.ai/".into()),
        iam: Some("https://iam.console.greennode.ai/".into()),
        storage: Some("https://vstorage.console.greennode.ai/".into()),
        cdn: Some("https://vcdn-api.vngcloud.vn/vcdn-api/".into()),
        vks: match region {
            "hcm-3" => Some("https://vks.console.greennode.ai/vks-api/".into()),
            "han-1" => Some("https://vks-han-1.console.greennode.ai/vks-api/".into()),
            _ => None,
        },
        vserver_backup: matches!(region, "hcm-3" | "han-1")
            .then(|| format!("https://{region}.console.greennode.ai/vserver/vbackup-gateway/")),
        backup_center: matches!(region, "hcm-3" | "han-1")
            .then(|| format!("https://{region}.api.vngcloud.vn/vbackup-gateway/")),
        database: Some("https://vdb.console.greennode.ai/vdb/".into()),
    };
    macro_rules! apply {
        ($($field:ident),+ $(,)?) => { $(
            if let Some(value) = overrides.$field.filter(|value| !value.is_empty()) { set.$field = Some(value); }
        )+ };
    }
    apply!(
        vserver,
        vlb,
        vnetwork,
        global_load_balancer,
        dns,
        container_registry,
        portal,
        signin,
        dashboard,
        token,
        billing,
        cdn_docs,
        monitor,
        iam,
        storage,
        cdn,
        vks,
        database
    );
    if set.vserver_backup.is_some() {
        apply!(vserver_backup);
    }
    if set.backup_center.is_some() {
        apply!(backup_center);
    }
    macro_rules! normalize {
        ($($field:ident),+ $(,)?) => { $(
            if let Some(value) = &mut set.$field {
                if !value.is_empty() && !value.ends_with('/') { value.push('/'); }
            }
        )+ };
    }
    normalize!(
        vserver,
        vlb,
        vnetwork,
        global_load_balancer,
        dns,
        container_registry,
        portal,
        dashboard,
        billing,
        monitor,
        iam,
        storage,
        vks,
        cdn,
        vserver_backup,
        backup_center,
        database
    );
    if let Some(signin) = &mut set.signin {
        signin.truncate(signin.trim_end_matches('/').len());
    }
    macro_rules! resolve {
        ($($field:ident),+ $(,)?) => {
            Ok(EndpointSet { $($field: set.$field.as_deref().map(Base::parse).transpose().map_err(|_| ())?,)+ })
        };
    }
    resolve!(
        vserver,
        vlb,
        vnetwork,
        global_load_balancer,
        dns,
        container_registry,
        portal,
        signin,
        dashboard,
        token,
        billing,
        cdn_docs,
        monitor,
        iam,
        storage,
        cdn,
        vks,
        vserver_backup,
        backup_center,
        database
    )
}

fn valid_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= 63
        && !region.starts_with('-')
        && !region.ends_with('-')
        && region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

pub(crate) fn vnetwork_regional_gateway(region: &str) -> Option<Base> {
    if !valid_region(region) {
        return None;
    }
    Base::parse(&format!(
        "https://{region}-vnetwork.console.greennode.ai/vnetwork-gateway/"
    ))
    .ok()
}

fn valid_path(path: &str) -> bool {
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            if !bytes.next().is_some_and(|b| b.is_ascii_hexdigit())
                || !bytes.next().is_some_and(|b| b.is_ascii_hexdigit())
            {
                return false;
            }
        } else if !byte.is_ascii_alphanumeric()
            && !matches!(
                byte,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
                    | b'/'
            )
        {
            return false;
        }
    }
    true
}

impl Base {
    pub(crate) fn parse(input: &str) -> Result<Self, BaseError> {
        if input
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control() || b == b'\\')
        {
            return Err(BaseError::InvalidUrl);
        }
        let (scheme, rest) = input.split_once("://").ok_or(BaseError::InvalidUrl)?;
        if !scheme.eq_ignore_ascii_case("https") {
            return Err(BaseError::InvalidUrl);
        }
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .ok_or(BaseError::InvalidUrl)?;
        if authority.is_empty() || authority.contains('@') {
            return Err(BaseError::InvalidUrl);
        }
        if authority.ends_with(':') {
            return Err(BaseError::InvalidPort);
        }
        let path = &rest[authority.len()..];
        if !valid_path(path) {
            return Err(BaseError::InvalidUrl);
        }
        if path.split('/').any(|segment| {
            let dots = segment.to_ascii_lowercase().replace("%2e", ".");
            dots == "." || dots == ".."
        }) {
            return Err(BaseError::InvalidUrl);
        }
        let url = Url::parse(input).map_err(|error| match error {
            url::ParseError::InvalidPort => BaseError::InvalidPort,
            _ => BaseError::InvalidUrl,
        })?;
        if let Some(url::Host::Ipv4(address)) = url.host() {
            let host = authority.split(':').next().ok_or(BaseError::InvalidUrl)?;
            if host != address.to_string() {
                return Err(BaseError::InvalidUrl);
            }
        }
        if url.scheme() != "https"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.host().is_none()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(BaseError::InvalidUrl);
        }
        let serialized = format!("{}{path}", &url[..url::Position::BeforePath]);
        Ok(Self { url, serialized })
    }

    pub(crate) fn same_origin(&self, other: &Self) -> bool {
        self.url.host() == other.url.host()
            && self.url.port_or_known_default() == other.url.port_or_known_default()
    }

    pub(crate) fn route(&self, template: &str, segments: &[&str]) -> Result<String, ()> {
        let mut route = self.serialized.clone();
        if !route.ends_with('/') {
            route.push('/');
        }
        route.push_str(template.trim_matches('/'));
        for segment in segments {
            if segment.is_empty() || *segment == "." || *segment == ".." || segment.contains('/') {
                return Err(());
            }
            route.push('/');
            for byte in segment.bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    route.push(char::from(byte));
                } else {
                    use std::fmt::Write;
                    write!(route, "%{byte:02X}").map_err(|_| ())?;
                }
            }
        }
        Ok(route)
    }
}

#[cfg(test)]
mod tests {
    use super::{Base, BaseError, EndpointOverrides, defaults, vnetwork_regional_gateway};

    #[test]
    fn resolver_returns_punycode_host() {
        let input = EndpointOverrides {
            vserver: Some("https://bücher.example/path/".into()),
            ..Default::default()
        };
        let resolved = defaults("hcm-3", input).unwrap();
        assert_eq!(
            resolved.vserver.as_deref(),
            Some("https://xn--bcher-kva.example/path/")
        );
    }

    #[test]
    fn rejects_dot_segments_in_base_paths() {
        for path in [
            "/prefix/./",
            "/prefix/../",
            "/prefix/%2e/",
            "/prefix/%2E%2e/",
            "/prefix/.%2e/",
            "/prefix/%2e./",
        ] {
            assert!(
                Base::parse(&format!("https://example.test{path}")).is_err(),
                "{path}"
            );
        }
        assert!(Base::parse("https://example.test/prefix/a.b/%252e/").is_ok());
    }

    #[test]
    fn rejects_non_ascii_base_path() {
        assert!(Base::parse("https://example.test/caf%C3%A9/").is_ok());
        assert!(Base::parse("https://example.test/café/").is_err());
    }

    #[test]
    fn rejects_malformed_path_escape() {
        assert!(Base::parse("https://example.test/a%25b/").is_ok());
        for path in ["/bad%/", "/bad%2/", "/bad%GG/"] {
            assert!(Base::parse(&format!("https://example.test{path}")).is_err());
        }
    }

    #[test]
    fn rejects_illegal_path_characters() {
        assert!(Base::parse("https://example.test/a-._~!$&'()*+,;=:@/b").is_ok());
        for byte in [
            b' ', 0x1f, b'\x7f', b'"', b'<', b'>', b'\\', b'^', b'`', b'{', b'|', b'}', b'[', b']',
        ] {
            let input = format!("https://example.test/a{}b", char::from(byte));
            assert!(Base::parse(&input).is_err());
        }
    }

    #[test]
    fn preserves_raw_base_path() {
        let base = Base::parse("https://example.test/prefix//caf%C3%A9/%7e/").unwrap();
        assert_eq!(
            base.route("items", &["id"]).unwrap(),
            "https://example.test/prefix//caf%C3%A9/%7e/items/id"
        );
    }

    #[test]
    fn rejects_extra_authority_slash() {
        assert!(Base::parse("https://example.test/").is_ok());
        for input in [
            "https:///user@example.test/",
            "https:///example.test:/",
            "https:///example.test/",
        ] {
            assert!(Base::parse(input).is_err());
        }
    }

    #[test]
    fn requires_canonical_ipv4() {
        assert!(Base::parse("https://127.0.0.1/").is_ok());
        assert!(Base::parse("https://example.test/").is_ok());
        for input in [
            "https://127.1/",
            "https://0177.0.0.1/",
            "https://0x7f.0.0.1/",
            "https://127.00.0.1/",
        ] {
            assert!(Base::parse(input).is_err(), "{input}");
        }
    }

    #[test]
    fn route_preserves_base_slashes() {
        for (base, expected) in [
            (
                "https://example.test/prefix//",
                "https://example.test/prefix//items/id",
            ),
            (
                "https://example.test/prefix",
                "https://example.test/prefix/items/id",
            ),
        ] {
            assert_eq!(
                Base::parse(base).unwrap().route("items", &["id"]).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn invalid_override_rejected() {
        let good = EndpointOverrides {
            vserver: Some("https://example.test/".into()),
            ..Default::default()
        };
        assert!(defaults("hcm-3", good).is_ok());
        let bad = EndpointOverrides {
            vserver: Some("http://example.test/".into()),
            ..Default::default()
        };
        assert!(defaults("hcm-3", bad).is_err());
    }

    #[test]
    fn regional_fallback() {
        assert_eq!(
            vnetwork_regional_gateway("hcm-3").as_deref(),
            Some("https://hcm-3-vnetwork.console.greennode.ai/vnetwork-gateway/")
        );
        assert!(vnetwork_regional_gateway("bad.region").is_none());
    }

    #[test]
    fn normalize_overrides() {
        let set = defaults(
            "hcm-3",
            EndpointOverrides {
                vserver: Some("https://example.test/path".into()),
                signin: Some("https://example.test///".into()),
                cdn_docs: Some("https://example.test/faq".into()),
                token: Some("https://example.test/token".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(set.vserver.as_deref(), Some("https://example.test/path/"));
        assert_eq!(set.signin.as_deref(), Some("https://example.test"));
        assert_eq!(set.cdn_docs.as_deref(), Some("https://example.test/faq"));
        assert_eq!(set.token.as_deref(), Some("https://example.test/token"));
    }

    #[test]
    fn field_defaults_and_override_precedence() {
        macro_rules! field {
            ($name:ident, $hcm:expr, $han:expr) => {{
                for (region, expected) in [("hcm-3", $hcm), ("han-1", $han)] {
                    let result = defaults(region, EndpointOverrides::default()).unwrap();
                    assert_eq!(
                        result.$name.as_deref(),
                        expected,
                        "{} {region}",
                        stringify!($name)
                    );
                    let overrides = EndpointOverrides {
                        $name: Some("https://override.test/".into()),
                        ..Default::default()
                    };
                    let actual = defaults(region, overrides).unwrap();
                    let expected_override = if stringify!($name) == "signin" {
                        "https://override.test"
                    } else {
                        "https://override.test/"
                    };
                    assert_eq!(
                        actual.$name.as_deref(),
                        Some(expected_override),
                        "{} {region}",
                        stringify!($name)
                    );
                }
            }};
        }
        field!(
            vserver,
            Some("https://hcm-3.console.greennode.ai/vserver/iam-vserver-gateway/"),
            Some("https://han-1.console.greennode.ai/vserver/iam-vserver-gateway/")
        );
        field!(
            vlb,
            Some("https://hcm-3.console.greennode.ai/vserver/iam-vlb-gateway/"),
            Some("https://han-1.console.greennode.ai/vserver/iam-vlb-gateway/")
        );
        field!(
            vnetwork,
            Some("https://hcm-3.console.greennode.ai/vserver/vnetwork-gateway/"),
            Some("https://han-1.console.greennode.ai/vserver/vnetwork-gateway/")
        );
        field!(
            portal,
            Some("https://hcm-3.console.greennode.ai/vserver/iam-billing-gateway/"),
            Some("https://han-1.console.greennode.ai/vserver/iam-billing-gateway/")
        );
        field!(
            global_load_balancer,
            Some("https://glb.console.greennode.ai/glb-controller/"),
            Some("https://glb.console.greennode.ai/glb-controller/")
        );
        field!(
            dns,
            Some("https://vdns.console.greennode.ai/vdns-api/"),
            Some("https://vdns.console.greennode.ai/vdns-api/")
        );
        field!(
            container_registry,
            Some("https://vcr.console.greennode.ai/vcr-api/"),
            Some("https://vcr.console.greennode.ai/vcr-api/")
        );
        field!(
            signin,
            Some("https://signin.greennode.ai"),
            Some("https://signin.greennode.ai")
        );
        field!(
            dashboard,
            Some("https://dashboard.console.greennode.ai/"),
            Some("https://dashboard.console.greennode.ai/")
        );
        field!(
            token,
            Some("https://dashboard.console.greennode.ai/accounts-api/v1/auth/token"),
            Some("https://dashboard.console.greennode.ai/accounts-api/v1/auth/token")
        );
        field!(
            billing,
            Some("https://dashboard.console.greennode.ai/"),
            Some("https://dashboard.console.greennode.ai/")
        );
        field!(
            cdn_docs,
            Some("https://docs.greennode.ai/faq/vcdn"),
            Some("https://docs.greennode.ai/faq/vcdn")
        );
        field!(
            monitor,
            Some("https://vmonitor.console.greennode.ai/"),
            Some("https://vmonitor.console.greennode.ai/")
        );
        field!(
            iam,
            Some("https://iam.console.greennode.ai/"),
            Some("https://iam.console.greennode.ai/")
        );
        field!(
            storage,
            Some("https://vstorage.console.greennode.ai/"),
            Some("https://vstorage.console.greennode.ai/")
        );
        field!(
            cdn,
            Some("https://vcdn-api.vngcloud.vn/vcdn-api/"),
            Some("https://vcdn-api.vngcloud.vn/vcdn-api/")
        );
        field!(
            vks,
            Some("https://vks.console.greennode.ai/vks-api/"),
            Some("https://vks-han-1.console.greennode.ai/vks-api/")
        );
        field!(
            vserver_backup,
            Some("https://hcm-3.console.greennode.ai/vserver/vbackup-gateway/"),
            Some("https://han-1.console.greennode.ai/vserver/vbackup-gateway/")
        );
        field!(
            backup_center,
            Some("https://hcm-3.api.vngcloud.vn/vbackup-gateway/"),
            Some("https://han-1.api.vngcloud.vn/vbackup-gateway/")
        );
        field!(
            database,
            Some("https://vdb.console.greennode.ai/vdb/"),
            Some("https://vdb.console.greennode.ai/vdb/")
        );
    }

    #[test]
    fn backup_overrides_cannot_enable_unmapped_region() {
        let set = defaults(
            "other-1",
            EndpointOverrides {
                vserver_backup: Some("https://override.test/".into()),
                backup_center: Some("https://override.test/".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(set.vserver_backup.is_none());
        assert!(set.backup_center.is_none());
    }

    #[test]
    fn dashboard_sets_token_but_not_billing() {
        let input = EndpointOverrides {
            dashboard: Some("https://dashboard.test/".into()),
            ..Default::default()
        };
        let set = defaults("hcm-3", input).unwrap();
        assert_eq!(
            set.token.as_deref(),
            Some("https://dashboard.test/accounts-api/v1/auth/token")
        );
        assert_eq!(
            set.billing.as_deref(),
            Some("https://dashboard.console.greennode.ai/")
        );
        let input = EndpointOverrides {
            dashboard: Some("https://dashboard.test/".into()),
            token: Some("https://token.test/exchange".into()),
            billing: Some("https://billing.test".into()),
            ..Default::default()
        };
        let set = defaults("hcm-3", input).unwrap();
        assert_eq!(set.token.as_deref(), Some("https://token.test/exchange"));
        assert_eq!(set.billing.as_deref(), Some("https://billing.test/"));
    }

    #[test]
    fn empty_override_uses_default() {
        let input = EndpointOverrides {
            vserver: Some(String::new()),
            dashboard: Some(String::new()),
            ..Default::default()
        };
        let set = defaults("hcm-3", input).unwrap();
        assert_eq!(
            set.dashboard.as_deref(),
            Some("https://dashboard.console.greennode.ai/")
        );
        assert_eq!(
            set.vserver.as_deref(),
            Some("https://hcm-3.console.greennode.ai/vserver/iam-vserver-gateway/")
        );
    }

    #[test]
    fn default_dashboard_and_override() {
        let default = defaults("hcm-3", EndpointOverrides::default()).unwrap();
        assert_eq!(
            default.dashboard.as_deref(),
            Some("https://dashboard.console.greennode.ai/")
        );
        let custom = EndpointOverrides {
            dashboard: Some("https://example.test/".into()),
            ..Default::default()
        };
        assert_eq!(
            defaults("hcm-3", custom).unwrap().dashboard.as_deref(),
            Some("https://example.test/")
        );
    }

    #[test]
    fn origin_normalizes_host_and_default_port() {
        let a = Base::parse("https://EXAMPLE.test:443/").unwrap();
        let b = Base::parse("https://example.test/").unwrap();
        let c = Base::parse("https://example.test:444/").unwrap();
        let d = Base::parse("https://other.test:443/").unwrap();
        assert!(a.same_origin(&b));
        assert!(!a.same_origin(&c));
        assert!(!a.same_origin(&d));
    }

    #[test]
    fn rejects_invalid_port() {
        assert!(Base::parse("https://example.test:443/").is_ok());
        for url in [
            "https://example.test:65536/",
            "https://example.test:abc/",
            "https://example.test:/",
        ] {
            assert!(matches!(Base::parse(url), Err(BaseError::InvalidPort)));
        }
    }

    #[test]
    fn https_only() {
        assert!(Base::parse("https://example.test").is_ok());
        assert!(Base::parse("http://example.test").is_err());
    }

    #[test]
    fn no_userinfo() {
        assert!(Base::parse("https://example.test").is_ok());
        assert!(Base::parse("https://user@example.test").is_err());
        assert!(Base::parse("https://@example.test").is_err());
    }

    #[test]
    fn no_fragment() {
        assert!(Base::parse("https://example.test").is_ok());
        assert!(Base::parse("https://example.test/#").is_err());
    }

    #[test]
    fn no_query() {
        assert!(Base::parse("https://example.test").is_ok());
        assert!(Base::parse("https://example.test/?").is_err());
    }

    #[test]
    fn no_empty_segment() {
        let base = Base::parse("https://example.test/").unwrap();
        assert!(base.route("items", &["one"]).is_ok());
        assert!(base.route("items", &[""]).is_err());
    }

    #[test]
    fn no_dot_segment() {
        let base = Base::parse("https://example.test/").unwrap();
        assert!(base.route("items", &["a.b"]).is_ok());
        assert!(base.route("items", &["."]).is_err());
    }

    #[test]
    fn no_dot_dot_segment() {
        let base = Base::parse("https://example.test/").unwrap();
        assert!(base.route("items", &["a..b"]).is_ok());
        assert!(base.route("items", &[".."]).is_err());
    }

    #[test]
    fn no_slash_segment() {
        let base = Base::parse("https://example.test/").unwrap();
        assert!(base.route("items", &["ab"]).is_ok());
        assert!(base.route("items", &["a/b"]).is_err());
    }

    #[test]
    fn encodes_segments() {
        let base = Base::parse("https://example.test/").unwrap();
        assert_eq!(
            base.route("items", &["a b%"]),
            Ok("https://example.test/items/a%20b%25".to_owned())
        );
    }
}
