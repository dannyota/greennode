use std::collections::BTreeMap;

use serde_json::value::RawValue;

use super::super::{Client, ResponseField, wire::failure};
use super::wire::{Object, Reader, array, integer};
use super::{Screen, output, unique};
use crate::wire::product::options::PathIds;
use crate::{Error, Failure, RequestContext};

/// Input for reading the synthetic quota catalogs.
#[derive(Clone, Copy, Debug, Default)]
pub struct ListSyntheticPackagesInput;

output! {
    /// Synthetic quota catalogs and billing settings.
    struct ListSyntheticPackagesOutput {
        /// Synthetic classes, sorted by priority, name, and identifier.
        items: Vec<SyntheticQuotaClass> => "Items",
        /// Packages from the v1 catalog, sorted by name and identifier.
        packages: Vec<SyntheticPackage> => "Packages",
        /// Billing periods from settings, in provider order.
        month_periods: Vec<ResponseField<i64>> => "MonthPeriods",
        /// Maximum monthly period for free quotas.
        max_month_period_for_free_quota: ResponseField<i64> => "MaxMonthPeriodForFreeQuota",
        /// Billing payment method.
        payment_method: String => "PaymentMethod",
    }
}

output! {
    /// A synthetic quota class and its available options and monthly units.
    struct SyntheticQuotaClass {
        /// Class identifier.
        id: String => "ID",
        /// Class name.
        name: String => "Name",
        /// Availability reported by the provider.
        status: String => "Status",
        /// Class description.
        description: String => "Description",
        /// Sort priority.
        priority: ResponseField<i64> => "Priority",
        /// Class options.
        options: Vec<SyntheticClassOption> => "Options",
        /// Purchasable units within this class.
        packages: Vec<SyntheticClassPackage> => "Packages",
    }
}

output! {
    /// A monthly unit package within one class.
    struct SyntheticClassPackage {
        /// Unit package identifier.
        id: String => "ID",
        /// Unit package name.
        name: String => "Name",
        /// Tier display name.
        tier_name: String => "TierName",
        /// Tier identifier.
        tier_id: ResponseField<i64> => "TierID",
        /// API units.
        api: ResponseField<i64> => "API",
        /// Location units.
        location: ResponseField<i64> => "Location",
        /// Monthly unit price in VND, if known.
        price: Option<ResponseField<f64>> => "Price",
    }
}

output! {
    /// A class option and its evidenced billing periods.
    struct SyntheticClassOption {
        /// Class package identifier.
        package_id: String => "PackageID",
        /// Number of locations.
        location: ResponseField<i64> => "Location",
        /// Minimum API count.
        min_api: ResponseField<i64> => "MinAPI",
        /// Maximum API count.
        max_api: ResponseField<i64> => "MaxAPI",
        /// API count increment.
        step: ResponseField<i64> => "Step",
        /// Price in VND when known.
        price: Option<ResponseField<f64>> => "Price",
        /// Eligible periods, in months.
        month_periods: Vec<ResponseField<i64>> => "MonthPeriods",
        /// Auto-renew eligibility where evidenced.
        auto_renew_eligible: Option<ResponseField<bool>> => "AutoRenewEligible",
    }
}

output! {
    /// A package from the v1 synthetic catalog.
    struct SyntheticPackage {
        /// V1 package identifier.
        id: String => "ID",
        /// Package name.
        name: String => "Name",
        /// Tier identifier.
        tier_id: ResponseField<i64> => "TierID",
        /// Number of API checks.
        api: ResponseField<i64> => "API",
        /// Number of locations.
        location: ResponseField<i64> => "Location",
        /// Price in VND when known.
        price: Option<ResponseField<f64>> => "Price",
        /// Eligible periods, in months.
        month_periods: Vec<ResponseField<i64>> => "MonthPeriods",
        /// Auto-renew eligibility where evidenced.
        auto_renew_eligible: Option<ResponseField<bool>> => "AutoRenewEligible",
    }
}

pub(super) const ACTIVE: &str = "ACTIVE";
const CLASSES: &str = "billing-api/v2/synthetic/quota-class";
const CLASS_PACKAGES: &str = "billing-api/v2/synthetic/quota-class/{}/packages";
const V1_PACKAGES: &str = "billing-api/v1/synthetic/packages";
const SETTINGS: &str = "billing-api/v1/settings";

impl Client {
    /// Reads synthetic classes, packages, and settings without ordering a quota.
    pub async fn list_synthetic_packages(
        &self,
        ctx: &RequestContext,
        _: ListSyntheticPackagesInput,
    ) -> Result<ListSyntheticPackagesOutput, Failure<ListSyntheticPackagesOutput>> {
        let mut reader = Reader::new(self, ctx, "monitor.ListSyntheticPackages");
        let mut classes = read_classes(&mut reader).await.map_err(failure)?;
        let raw = reader.get(V1_PACKAGES, &[]).await.map_err(failure)?;
        let mut packages = reader.decode(v1_packages(&raw)).map_err(failure)?;
        let settings = read_settings(&mut reader).await.map_err(failure)?;
        read_class_packages(&mut reader, &mut classes)
            .await
            .map_err(failure)?;
        if class_ids(&classes)
            .chain(packages.iter().map(|package| package.id.as_str()))
            .any(|id| reader.evidence.id_reflects(id))
        {
            return Err(failure(reader.invalid()));
        }
        let offers = offers(&classes);
        let periods = settings.periods(true);
        let free_options: Vec<Vec<_>> = classes
            .iter()
            .map(|class| {
                class
                    .options
                    .iter()
                    .map(|option| {
                        offers
                            .get(option.output.package_id.as_str())
                            .is_some_and(|offer| {
                                offer.free(
                                    &class.id,
                                    get(option.output.location).unwrap_or_default(),
                                    None,
                                )
                            })
                    })
                    .collect()
            })
            .collect();
        for package in &mut packages {
            if let Some(ResponseField::Value(price)) = package.price {
                package.month_periods = settings.periods(price == 0.0);
            }
        }
        packages.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        let items = classes
            .into_iter()
            .zip(free_options)
            .map(|(class, eligible)| SyntheticQuotaClass {
                id: class.id,
                name: class.name,
                status: class.status,
                description: class.description,
                priority: ResponseField::Value(class.priority),
                packages: class.packages,
                options: class
                    .options
                    .into_iter()
                    .zip(eligible)
                    .map(|(option, free)| {
                        let mut output = option.output;
                        if free {
                            output.month_periods.clone_from(&periods);
                        }
                        output
                    })
                    .collect(),
            })
            .collect();
        let mut output = ListSyntheticPackagesOutput {
            items,
            packages,
            month_periods: settings.periods(false),
            max_month_period_for_free_quota: ResponseField::Value(settings.maximum),
            payment_method: settings.payment_method,
        };
        if !output.screen(&reader.evidence) {
            return Err(failure(reader.invalid()));
        }
        Ok(output)
    }
}

pub(super) struct Class {
    id: String,
    name: String,
    status: String,
    description: String,
    priority: i64,
    options: Vec<ClassOption>,
    packages: Vec<SyntheticClassPackage>,
}

struct ClassOption {
    output: SyntheticClassOption,
    status: String,
}

pub(super) struct Settings {
    periods: Vec<i64>,
    maximum: i64,
    payment_method: String,
}

impl Settings {
    fn periods(&self, free: bool) -> Vec<ResponseField<i64>> {
        self.periods
            .iter()
            .copied()
            .filter(|period| !free || *period <= self.maximum)
            .map(ResponseField::Value)
            .collect()
    }
}

pub(super) async fn read_classes(reader: &mut Reader<'_>) -> Result<Vec<Class>, Error> {
    let raw = reader.get(CLASSES, &[]).await?;
    reader.decode(classes(&raw))
}

pub(super) async fn read_settings(reader: &mut Reader<'_>) -> Result<Settings, Error> {
    let raw = reader.get(SETTINGS, &[]).await?;
    reader.decode(settings(&raw))
}

pub(super) async fn read_class_packages(
    reader: &mut Reader<'_>,
    classes: &mut [Class],
) -> Result<(), Error> {
    if classes.len() > 64
        || classes.iter().any(|class| {
            !PathIds::Strict.allows(&class.id) || reader.evidence.id_reflects(&class.id)
        })
    {
        return Err(reader.invalid());
    }
    classes.sort_by(|a, b| (a.priority, &a.name, &a.id).cmp(&(b.priority, &b.name, &b.id)));
    for class in classes.iter_mut() {
        if reader.evidence.id_reflects(&class.id) {
            return Err(reader.invalid());
        }
        let raw = reader.get(CLASS_PACKAGES, &[&class.id]).await?;
        class.packages = reader.decode(class_packages(&raw))?;
    }
    reader
        .decision_ids
        .extend(class_ids(classes).map(str::to_owned));
    Ok(())
}

fn classes(raw: &RawValue) -> Option<Vec<Class>> {
    let mut result = Vec::new();
    for row in array(raw)? {
        let o = Object::of(row)?;
        let status = o.text("status")?;
        let mut options = match o.field("config")? {
            Some(value) => class_options(value)?,
            None if status != ACTIVE => Vec::new(),
            None => return None,
        };
        options.sort_by(|a, b| {
            (get(a.output.location), &a.output.package_id)
                .cmp(&(get(b.output.location), &b.output.package_id))
        });
        result.push(Class {
            id: o.text("id")?,
            name: o.text("name")?,
            status,
            description: o.text("description")?,
            priority: o.integer("priority")?,
            options,
            packages: Vec::new(),
        });
    }
    (result.iter().all(|class| !class.id.is_empty())
        && unique(result.iter().map(|class| &class.id)))
    .then_some(result)
}

fn class_options(raw: &RawValue) -> Option<Vec<ClassOption>> {
    let config = Object::of(raw)?;
    if config.text("packages")?.is_empty() {
        return None;
    }
    let mut options = Vec::new();
    for row in array(config.required("locations")?)? {
        let o = Object::of(row)?;
        let package_id = o.text("packageId")?;
        let (location, min, max, step) = (
            o.integer("amount")?,
            o.integer("minApi")?,
            o.integer("maxApi")?,
            o.integer("step")?,
        );
        if !PathIds::WithUnderscore.allows(&package_id)
            || location <= 0
            || min <= 0
            || max < min
            || step <= 0
        {
            return None;
        }
        options.push(ClassOption {
            output: SyntheticClassOption {
                package_id,
                location: ResponseField::Value(location),
                min_api: ResponseField::Value(min),
                max_api: ResponseField::Value(max),
                step: ResponseField::Value(step),
                price: None,
                month_periods: Vec::new(),
                auto_renew_eligible: None,
            },
            status: o.text("status")?,
        });
    }
    unique(
        options
            .iter()
            .map(|option| (&option.output.package_id, get(option.output.location))),
    )
    .then_some(options)
}

fn class_packages(raw: &RawValue) -> Option<Vec<SyntheticClassPackage>> {
    let mut packages = Vec::new();
    for row in array(raw)? {
        let o = Object::of(row)?;
        let id = o.text("id")?;
        let (tier, api, location) = (
            o.integer("tierId")?,
            o.integer("api")?,
            o.integer("location")?,
        );
        if !PathIds::WithUnderscore.allows(&id) || tier < 0 || api <= 0 || location <= 0 {
            return None;
        }
        packages.push(SyntheticClassPackage {
            id,
            name: o.text("name")?,
            tier_name: o.text("tierName")?,
            tier_id: ResponseField::Value(tier),
            api: ResponseField::Value(api),
            location: ResponseField::Value(location),
            price: o.price()?.map(ResponseField::Value),
        });
    }
    packages.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
    unique(packages.iter().map(|package| &package.id)).then_some(packages)
}

fn v1_packages(raw: &RawValue) -> Option<Vec<SyntheticPackage>> {
    let mut packages = Vec::new();
    for row in array(raw)? {
        let o = Object::of(row)?;
        let id = o.text("id")?;
        let (tier, api, location) = (
            o.integer("tierId")?,
            o.integer("api")?,
            o.integer("location")?,
        );
        if !PathIds::WithUnderscore.allows(&id) || tier < 0 || api < 0 || location < 0 {
            return None;
        }
        packages.push(SyntheticPackage {
            id,
            name: o.text("name")?,
            tier_id: ResponseField::Value(tier),
            api: ResponseField::Value(api),
            location: ResponseField::Value(location),
            price: o.price()?.map(ResponseField::Value),
            month_periods: Vec::new(),
            auto_renew_eligible: None,
        });
    }
    unique(packages.iter().map(|package| &package.id)).then_some(packages)
}

fn settings(raw: &RawValue) -> Option<Settings> {
    let o = Object::of(raw)?;
    let periods: Vec<i64> = array(o.required("monthPeriods")?)?
        .into_iter()
        .map(integer)
        .collect::<Option<_>>()?;
    let maximum = o.integer("maxMonthPeriodForFreeQuota")?;
    if maximum <= 0
        || periods.is_empty()
        || periods.iter().any(|period| *period <= 0)
        || !unique(&periods)
    {
        return None;
    }
    Some(Settings {
        periods,
        maximum,
        payment_method: o.text("paymentMethod")?,
    })
}

#[derive(Default)]
struct Offer<'a> {
    package: Option<(&'a Class, &'a SyntheticClassPackage)>,
    option: Option<(&'a Class, &'a ClassOption)>,
    ambiguity: bool,
}

fn offers(classes: &[Class]) -> BTreeMap<&str, Offer<'_>> {
    let mut offers: BTreeMap<&str, Offer<'_>> = BTreeMap::new();
    for class in classes {
        for package in &class.packages {
            let offer = offers.entry(&package.id).or_default();
            offer.ambiguity |= offer.package.replace((class, package)).is_some();
        }
        for option in &class.options {
            let offer = offers.entry(&option.output.package_id).or_default();
            offer.ambiguity |= offer.option.replace((class, option)).is_some();
        }
    }
    offers
}

impl Offer<'_> {
    fn free(&self, class_id: &str, location: i64, quantity: Option<i64>) -> bool {
        self.matches(class_id, location, quantity)
            && self
                .package
                .is_some_and(|(_, package)| package.price == Some(ResponseField::Value(0.0)))
    }

    fn matches(&self, class_id: &str, location: i64, quantity: Option<i64>) -> bool {
        let (Some((package_class, package)), Some((option_class, option))) =
            (self.package, self.option)
        else {
            return false;
        };
        let (Some(api), Some(min), Some(max), Some(step)) = (
            get(package.api),
            get(option.output.min_api),
            get(option.output.max_api),
            get(option.output.step),
        ) else {
            return false;
        };
        !self.ambiguity
            && package_class.id == class_id
            && option_class.id == class_id
            && package_class.status == ACTIVE
            && option.status == ACTIVE
            && get(package.location) == Some(location)
            && get(option.output.location) == Some(location)
            && quantity.is_none_or(|quantity| {
                (min..=max).contains(&quantity)
                    && (quantity - min) % step == 0
                    && quantity % api == 0
            })
    }
}

pub(super) struct QuoteOffer {
    pub(super) location: i64,
    pub(super) list_price: f64,
}

pub(super) enum OfferIssue {
    Class,
    Package,
    Option,
    Period,
}

pub(super) async fn quote_offer(
    reader: &mut Reader<'_>,
    class_id: &str,
    package_id: &str,
    quantity: i64,
) -> Result<Result<QuoteOffer, OfferIssue>, Error> {
    let mut classes = read_classes(reader).await?;
    read_class_packages(reader, &mut classes).await?;
    let reflected = |reader: &Reader<'_>| {
        [class_id, package_id]
            .into_iter()
            .chain(class_ids(&classes))
            .any(|id| reader.evidence.id_reflects(id))
    };
    if reflected(reader) {
        return Err(reader.invalid());
    }
    let Some(class) = classes
        .iter()
        .find(|class| class.id == class_id && class.status == ACTIVE)
    else {
        return Ok(Err(OfferIssue::Class));
    };
    let Some(package) = class
        .packages
        .iter()
        .find(|package| package.id == package_id)
    else {
        return Ok(Err(OfferIssue::Package));
    };
    let unit_price = reader.decode(package.price.and_then(get))?;
    let location = reader.decode(get(package.location))?;
    if get(package.api) != Some(1)
        || !offers(&classes)
            .get(package_id)
            .is_some_and(|offer| offer.matches(class_id, location, Some(quantity)))
    {
        return Ok(Err(OfferIssue::Option));
    }
    let list_price = unit_price * quantity as f64;
    if !list_price.is_finite() {
        return Err(reader.invalid());
    }
    let settings = read_settings(reader).await?;
    if !settings_offer_one_free_month(&settings) {
        return Ok(Err(OfferIssue::Period));
    }
    if reflected(reader) {
        return Err(reader.invalid());
    }
    Ok(Ok(QuoteOffer {
        location,
        list_price,
    }))
}

pub(super) fn settings_offer_one_free_month(settings: &Settings) -> bool {
    settings
        .periods
        .iter()
        .copied()
        .filter(|period| *period <= settings.maximum)
        .eq([1])
}

pub(super) fn status_eligibility(
    reader: &Reader<'_>,
    classes: &[Class],
    settings: &Settings,
    class_id: &str,
    package_id: &str,
    api: i64,
    location: i64,
) -> Result<bool, Error> {
    if [class_id, package_id]
        .into_iter()
        .chain(class_ids(classes))
        .any(|id| reader.evidence.id_reflects(id))
    {
        return Err(reader.invalid());
    }
    if let Some(package) = classes
        .iter()
        .filter(|class| class.id == class_id)
        .flat_map(|class| &class.packages)
        .find(|package| package.id == package_id)
    {
        if package.price.is_none() {
            return Err(reader.invalid());
        }
        if get(package.api) != Some(1) {
            return Ok(false);
        }
    }
    Ok(api > 0
        && settings_offer_one_free_month(settings)
        && offers(classes)
            .get(package_id)
            .is_some_and(|offer| offer.free(class_id, location, Some(api))))
}

fn get<T: Copy>(field: ResponseField<T>) -> Option<T> {
    match field {
        ResponseField::Value(value) => Some(value),
        ResponseField::Redacted => None,
    }
}

fn class_ids(classes: &[Class]) -> impl Iterator<Item = &str> {
    classes.iter().flat_map(|class| {
        std::iter::once(class.id.as_str())
            .chain(
                class
                    .options
                    .iter()
                    .map(|option| option.output.package_id.as_str()),
            )
            .chain(class.packages.iter().map(|package| package.id.as_str()))
    })
}
