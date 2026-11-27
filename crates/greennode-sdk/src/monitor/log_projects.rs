use super::inventory::{get_input, id, list_input, output, page_values, plain, strict};
use super::wire::{check_path_id, failure, input_error, requires_failure};
use super::{Client, ResponseField};
use crate::model::wire::{Field, WireField, model};
use crate::quote::request::{
    self, Product, Request,
    monitor::{LOG_QUOTE, is_log_price},
};
use crate::wire::product::evidence::Evidence;
use crate::wire::strict_json::{Shape, ShapeField};
use crate::{Error, Failure, List, PagedList, RequestContext};
use serde::Serialize;
use serde_json::json;

const GET: super::wire::Read = plain("monitor.GetLogProject", "log-api/v1/projects/{}");
const LIST: super::wire::Read = strict("monitor.ListLogProjects", "log-api/v1/projects");
const CLASSES: super::wire::Read = plain(
    "monitor.ListLogProjectClasses",
    "billing-api/v2/log/quota-class",
);
const QUOTE: super::wire::Read = plain(LOG_QUOTE, "billing-api/v2/log/prices/created-price");
const PAGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "content",
        shape: Shape::Array(&Shape::Any),
    },
    ShapeField {
        name: "currentPage",
        shape: Shape::Any,
    },
    ShapeField {
        name: "pageSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalPages",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalElements",
        shape: Shape::Any,
    },
]);
const GUARDED_PAGE: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "content",
        shape: Shape::Array(&PROJECT_GUARD),
    },
    ShapeField {
        name: "currentPage",
        shape: Shape::Any,
    },
    ShapeField {
        name: "pageSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalPages",
        shape: Shape::Any,
    },
    ShapeField {
        name: "totalElements",
        shape: Shape::Any,
    },
]);
const PROJECT_GUARD: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "description",
        shape: Shape::Any,
    },
    ShapeField {
        name: "status",
        shape: Shape::Any,
    },
    ShapeField {
        name: "billingStatus",
        shape: Shape::Any,
    },
    ShapeField {
        name: "projectType",
        shape: Shape::Any,
    },
    ShapeField {
        name: "createdAt",
        shape: Shape::Any,
    },
]);
const RETENTION_GUARD: Shape<'static> = Shape::Object(&[
    ShapeField {
        name: "amount",
        shape: Shape::Any,
    },
    ShapeField {
        name: "minSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "maxSize",
        shape: Shape::Any,
    },
    ShapeField {
        name: "step",
        shape: Shape::Any,
    },
    ShapeField {
        name: "packageId",
        shape: Shape::Any,
    },
]);
const CLASS_GUARD: Shape<'static> = Shape::Array(&Shape::Object(&[
    ShapeField {
        name: "id",
        shape: Shape::Any,
    },
    ShapeField {
        name: "name",
        shape: Shape::Any,
    },
    ShapeField {
        name: "description",
        shape: Shape::Any,
    },
    ShapeField {
        name: "priority",
        shape: Shape::Any,
    },
    ShapeField {
        name: "status",
        shape: Shape::Any,
    },
    ShapeField {
        name: "config",
        shape: Shape::Object(&[ShapeField {
            name: "retentions",
            shape: Shape::Array(&RETENTION_GUARD),
        }]),
    },
]));
get_input!(
    /// Selects a log project.
    GetLogProjectInput,
    /// Log project identifier.
    log_project_id
);
list_input!(
    /// Selects all log class offerings.
    ListLogProjectClassesInput
);
/// Filters and pagination for log projects.
#[derive(Clone, Debug, Default)]
pub struct ListLogProjectsInput {
    /// Name filter.
    pub query: String,
    /// Billing status filter.
    pub billing_status: String,
    /// Zero-based page.
    pub page: i64,
    /// Requested page size. Non-positive values select 100.
    pub size: i64,
}
/// One page of log projects.
pub type ListLogProjectsOutput = PagedList<LogProject>;
/// Available log project classes.
pub type ListLogProjectClassesOutput = List<LogProjectClass>;
/// A selected log project.
#[derive(Clone, Debug, Serialize)]
pub struct GetLogProjectOutput {
    /// The selected project.
    #[serde(rename = "LogProject")]
    pub log_project: LogProject,
}

model! { @ true;
    /// Log project inventory item.
    pub struct LogProject {
        /// Log project identifier.
        id:String=>"id","ID";
        /// Project name.
        project_name:String=>"name","ProjectName";
        /// Project description.
        project_description:String=>"description","ProjectDescription";
        /// Current provisioning status.
        status:String=>"status","Status";
        /// Current billing status.
        billing_status:String=>"billingStatus","BillingStatus";
        /// Provider project type.
        project_type:String=>"projectType","ProjectType";
        /// Provider creation timestamp text.
        created_at:String=>"createdAt","CreatedAt";
    }
}
model! { @ true;
    /// Retention option in a log project class.
    pub struct LogProjectRetention {
        /// Retention window in days.
        amount:ResponseField<i64> =>"amount","Amount";
        /// Minimum GB per day.
        min_size:ResponseField<i64> =>"minSize","MinSize";
        /// Maximum GB per day.
        max_size:ResponseField<i64> =>"maxSize","MaxSize";
        /// GB per day increment.
        step:ResponseField<i64> =>"step","Step";
        /// Billing package identifier.
        package_id:String=>"packageId","PackageID";
    }
}
model! { @ true; struct Retentions { items:Vec<LogProjectRetention> =>"retentions","Retentions"; } }
model! { @ true; struct ClassWire {
    id:String=>"id","ID"; name:String=>"name","Name"; description:String=>"description","Description";
    priority:ResponseField<i64> =>"priority","Priority"; status:String=>"status","Status"; config:Retentions=>"config","Config";
} }
/// Log project class and its retention offerings.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LogProjectClass {
    /// Class identifier.
    #[serde(rename = "ID")]
    pub id: String,
    /// Class name.
    pub name: String,
    /// Class description.
    pub description: String,
    /// Display priority.
    pub priority: ResponseField<i64>,
    /// Class availability.
    pub status: String,
    /// Retention offerings in this class.
    pub retentions: Vec<LogProjectRetention>,
}
model! { @ true; struct ProjectPage {
    items:Option<Vec<LogProject>> =>"content","Items"; page:Option<i64> =>"currentPage","Page"; size:Option<i64> =>"pageSize","PageSize";
    total:Option<i64> =>"totalElements","TotalItem"; pages:Option<i64> =>"totalPages","TotalPage";
}}

/// Selects the log project to price.
#[derive(Clone, Debug)]
pub struct CreateLogProjectInput {
    /// Project name.
    pub name: String,
    /// Project description.
    pub description: String,
    /// Class name. Empty selects Basic.
    pub class: String,
    /// Retention period. Zero selects the only option when unique.
    pub retention_days: i64,
    /// GB per day. Zero selects the minimum.
    pub gb_per_day: i64,
    /// Price guard for orders. The quote ignores it.
    pub max_price: f64,
    /// Settlement control for orders. The quote ignores it.
    pub no_wait: bool,
}
impl CreateLogProjectInput {
    /// Selects the project name to price.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: String::new(),
            class: String::new(),
            retention_days: 0,
            gb_per_day: 0,
            max_price: 0.0,
            no_wait: false,
        }
    }
}
model! { @ true;
    /// Price to create a log project, in VND.
    pub struct QuoteCreateLogProjectOutput {
        /// Price after discounts.
        optimum_price:ResponseField<f64> =>"optimumPrice","OptimumPrice";
        /// Base price.
        original_price:ResponseField<f64> =>"originalPrice","OriginalPrice";
        /// Discount amount.
        discount_price:ResponseField<f64> =>"discountPrice","DiscountPrice";
        /// Discount percentage, when reported.
        discount_percent:Option<ResponseField<f64>> =>"discountPercent","DiscountPercent";
        /// Components of the price.
        properties:Vec<LogProjectPriceProperty> =>"propertiesPrice","Properties";
    }
}
model! { @ true;
    /// Price component of a log project quote.
    pub struct LogProjectPriceProperty {
        /// Component name.
        name:String=>"name","Name";
        /// Component description, when reported.
        description:Option<String> =>"description","Description";
        /// Discounted amount.
        optimum_price:ResponseField<f64> =>"optimumPrice","OptimumPrice";
        /// Base amount.
        original_price:ResponseField<f64> =>"originalPrice","OriginalPrice";
        /// Discount amount.
        discount_price:ResponseField<f64> =>"discountPrice","DiscountPrice";
        /// Discount percentage, when reported.
        discount_percent:Option<ResponseField<f64>> =>"discountPercent","DiscountPercent";
    }
}

impl Client {
    /// Reads a log project by identifier.
    pub async fn get_log_project(
        &self,
        ctx: &RequestContext,
        input: GetLogProjectInput,
    ) -> Result<GetLogProjectOutput, Failure<GetLogProjectOutput>> {
        check_path_id(GET.id, "LogProjectID", &input.log_project_id)?;
        let mut evidence = Evidence::new();
        let raw = self
            .read(ctx, &GET, &[&input.log_project_id], &[], &mut evidence)
            .await
            .map_err(failure)?;
        let mut log_project = output::<Field<LogProject>>(&raw, &Shape::Any, &GET, &evidence)
            .map_err(failure)?
            .0;
        if !id(&mut log_project.id, &evidence) || !log_project.redact(&evidence) {
            return Err(failure(GET.invalid(&evidence)));
        }
        Ok(GetLogProjectOutput { log_project })
    }
    /// Lists one page of log projects.
    pub async fn list_log_projects(
        &self,
        ctx: &RequestContext,
        input: ListLogProjectsInput,
    ) -> Result<ListLogProjectsOutput, Failure<ListLogProjectsOutput>> {
        let mut evidence = Evidence::new();
        let (page, size) = page_values(input.page, input.size, 0, 100);
        let mut query = vec![("page", page.as_str()), ("size", size.as_str())];
        if !input.query.is_empty() {
            query.push(("query", &input.query));
        }
        if !input.billing_status.is_empty() {
            query.push(("billing_status", &input.billing_status));
        }
        let raw = self
            .read(ctx, &LIST, &[], &query, &mut evidence)
            .await
            .map_err(failure)?;
        let wire: ProjectPage = output(&raw, &PAGE, &LIST, &evidence).map_err(failure)?;
        let ProjectPage {
            items: Some(mut items),
            page: Some(mut page),
            size: Some(mut page_size),
            total: Some(mut total_item),
            pages: Some(mut total_page),
        } = wire
        else {
            return Err(failure(LIST.error(
                Some(200),
                "InvalidResponse",
                "name search was incomplete or ambiguous",
                false,
                &evidence,
            )));
        };
        if !items
            .iter_mut()
            .all(|item| id(&mut item.id, &evidence) && item.redact(&evidence))
            || ![&mut page, &mut page_size, &mut total_item, &mut total_page]
                .into_iter()
                .all(|n| n.redact(&evidence))
        {
            return Err(failure(LIST.invalid(&evidence)));
        }
        Ok(PagedList {
            items,
            page,
            page_size,
            total_item,
            total_page,
        })
    }
    /// Lists classes and retention options for log projects.
    pub async fn list_log_project_classes(
        &self,
        ctx: &RequestContext,
        _: ListLogProjectClassesInput,
    ) -> Result<ListLogProjectClassesOutput, Failure<ListLogProjectClassesOutput>> {
        let mut evidence = Evidence::new();
        let mut classes = self
            .log_classes(ctx, CLASSES.id, &mut evidence)
            .await
            .map_err(failure)?;
        if !classes.iter_mut().all(|class| {
            id(&mut class.id, &evidence)
                && [&mut class.name, &mut class.description, &mut class.status]
                    .into_iter()
                    .all(|text| text.redact(&evidence))
                && class.priority.redact(&evidence)
                && class
                    .config
                    .items
                    .iter_mut()
                    .all(|option| id(&mut option.package_id, &evidence) && option.redact(&evidence))
        }) {
            return Err(failure(CLASSES.invalid(&evidence)));
        }
        Ok(List {
            items: classes
                .into_iter()
                .map(|class| LogProjectClass {
                    id: class.id,
                    name: class.name,
                    description: class.description,
                    priority: class.priority,
                    status: class.status,
                    retentions: class.config.items,
                })
                .collect(),
        })
    }
    /// Quotes the cost to create a log project.
    pub async fn quote_create_log_project(
        &self,
        ctx: &RequestContext,
        input: CreateLogProjectInput,
    ) -> Result<QuoteCreateLogProjectOutput, Failure<QuoteCreateLogProjectOutput>> {
        if input.name.is_empty() {
            return Err(requires_failure(LOG_QUOTE, "Name"));
        }
        self.log_quote(ctx, input).await.map_err(failure)
    }
    async fn log_classes(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        evidence: &mut Evidence,
    ) -> Result<Vec<ClassWire>, Error> {
        let read = super::wire::Read { id: op, ..CLASSES };
        let raw = self.read(ctx, &read, &[], &[], evidence).await?;
        if is_log_price(op) {
            return Ok(super::wire::decode::<Field<Vec<ClassWire>>>(
                &raw,
                &CLASS_GUARD,
                &read,
                evidence,
            )?
            .0);
        }
        Ok(output::<Field<Vec<ClassWire>>>(&raw, &Shape::Any, &read, evidence)?.0)
    }
    async fn log_quote(
        &self,
        ctx: &RequestContext,
        input: CreateLogProjectInput,
    ) -> Result<QuoteCreateLogProjectOutput, Error> {
        let mut evidence = Evidence::new();
        let mut jitter = || self.session.retry_jitter();
        let runtime = self
            .session
            .runtime(ctx, &mut jitter)
            .map_err(|error| *error)?;
        if let Some(credentials) = runtime.configured_credentials {
            evidence.absorb(credentials);
        }
        if [&input.name, &input.description, &input.class]
            .into_iter()
            .any(|text| evidence.text_reflects(text))
        {
            return Err(input_error(format!(
                "{LOG_QUOTE} requires a safe Name, Description, and Class"
            )));
        }
        let terms = self
            .prepare_log_order(ctx, LOG_QUOTE, &input, &mut evidence)
            .await?;
        self.price_log_order(ctx, LOG_QUOTE, &terms, &mut evidence)
            .await
    }

    pub(super) async fn prepare_log_order(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        input: &CreateLogProjectInput,
        evidence: &mut Evidence,
    ) -> Result<LogOrder, Error> {
        let read = super::wire::Read { id: op, ..QUOTE };
        let classes = self.log_classes(ctx, op, evidence).await?;
        let name = if input.class.is_empty() {
            "Basic"
        } else {
            &input.class
        };
        if evidence.text_reflects(name) {
            return Err(read.invalid(evidence));
        }
        let quoted = crate::go_text::go_quote(name);
        let class = classes
            .iter()
            .find(|class| class.name == name)
            .filter(|class| class.status == "ACTIVE" && !class.config.items.is_empty())
            .ok_or_else(|| input_error(format!("{op} class {quoted} is not an orderable class")))?;
        let options = &class.config.items;
        let retention = if input.retention_days == 0 {
            let [only] = options.as_slice() else {
                return Err(input_error(format!(
                    "{op} class {quoted} has {} retention options, RetentionDays must name one",
                    options.len()
                )));
            };
            only
        } else {
            options
                .iter()
                .find(|option| option.amount == ResponseField::Value(input.retention_days))
                .ok_or_else(|| {
                    input_error(format!(
                        "{op} class {quoted} has no {}-day retention option",
                        input.retention_days
                    ))
                })?
        };
        let (ResponseField::Value(days), ResponseField::Value(minimum)) =
            (retention.amount, retention.min_size)
        else {
            return Err(read.invalid(evidence));
        };
        let quantity = if input.gb_per_day == 0 {
            minimum
        } else {
            input.gb_per_day
        }
        .checked_mul(days)
        .ok_or_else(|| input_error(format!("{op} quantity overflows")))?;
        let ids = [class.id.as_str(), retention.package_id.as_str()];
        if ids.iter().any(|id| {
            !crate::wire::product::options::PathIds::WithUnderscore.allows(id)
                || evidence.id_reflects(id)
        }) {
            return Err(read.invalid(evidence));
        }
        let body = json!({"redirectUrl":"https://vmonitor.console.vngcloud.vn/quota-usages/log","packageId":retention.package_id,"quantity":quantity,"buyWith":{},"monthPeriod":1,"projectName":input.name,"projectDescription":input.description,"pay":true});
        Ok(LogOrder {
            body,
            class_id: class.id.clone(),
            package_id: retention.package_id.clone(),
        })
    }

    pub(super) async fn price_log_order(
        &self,
        ctx: &RequestContext,
        op: &'static str,
        terms: &LogOrder,
        evidence: &mut Evidence,
    ) -> Result<QuoteCreateLogProjectOutput, Error> {
        let read = super::wire::Read { id: op, ..QUOTE };
        let ids = [terms.class_id.as_str(), terms.package_id.as_str()];
        let raw = request::exchange(
            &self.session,
            ctx,
            evidence,
            Request {
                operation: op,
                template: QUOTE.route,
                product: Product::Monitor {
                    identifiers: &ids,
                    no_redirect: false,
                },
                path: &[],
                query: &[],
                body: Some(&terms.body),
            },
            None,
        )
        .await
        .map_err(|error| *error)?;
        let mut quote: QuoteCreateLogProjectOutput = output(&raw, &Shape::Any, &read, evidence)?;
        if ids.iter().any(|id| evidence.id_reflects(id)) || !quote.redact(evidence) {
            return Err(read.invalid(evidence));
        }
        Ok(quote)
    }
    pub(super) async fn log_project_page_guard(
        &self,
        ctx: &RequestContext,
        mutation: &super::mutation::Mutation,
        name: &str,
        page: i64,
        evidence: &mut Evidence,
    ) -> Result<PagedList<LogProject>, Error> {
        let read = super::wire::Read {
            id: mutation.id,
            ..LIST
        };
        let page = page.to_string();
        let query = [("page", page.as_str()), ("size", "100"), ("query", name)];
        let raw = self.read(ctx, &read, &[], &query, evidence).await?;
        let wire: ProjectPage = super::wire::decode(&raw, &GUARDED_PAGE, &read, evidence)?;
        let ProjectPage {
            items: Some(items),
            page: Some(page),
            size: Some(page_size),
            total: Some(total_item),
            pages: Some(total_page),
        } = wire
        else {
            return Err(super::settlement::name_failure(mutation, evidence));
        };
        if items
            .iter()
            .any(|project| !safe_project_guard(project, evidence))
        {
            return Err(mutation.invalid(200, evidence));
        }
        Ok(PagedList {
            items,
            page,
            page_size,
            total_item,
            total_page,
        })
    }
}

pub(super) struct LogOrder {
    pub(super) body: serde_json::Value,
    pub(super) class_id: String,
    pub(super) package_id: String,
}

pub(super) fn safe_project_guard(project: &LogProject, evidence: &Evidence) -> bool {
    !(evidence.id_reflects(&project.id)
        || [
            &project.project_name,
            &project.project_description,
            &project.status,
            &project.billing_status,
            &project.project_type,
            &project.created_at,
        ]
        .into_iter()
        .any(|value| evidence.text_reflects(value)))
}
