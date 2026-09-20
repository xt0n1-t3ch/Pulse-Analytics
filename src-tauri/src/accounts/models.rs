use codex_presence_core::{IndividualSpendLimit, RateLimitResetCreditsSummary};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct AccountIdentity {
    pub subject: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub organization: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountWindow {
    pub id: String,
    pub label: String,
    pub scope: String,
    pub duration_minutes: Option<u64>,
    pub used: Option<f64>,
    pub limit: Option<f64>,
    pub unit: Option<String>,
    pub used_percent: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountBalance {
    pub id: String,
    pub label: String,
    pub value: String,
    pub unit: String,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub cycle_ends_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AccountSnapshot {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub origin: String,
    pub identity: AccountIdentity,
    pub plan: Option<String>,
    pub status: String,
    pub observed_at: Option<String>,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub retry_at: Option<String>,
    pub windows: Vec<AccountWindow>,
    pub balances: Vec<AccountBalance>,
    pub reset_credits: Option<RateLimitResetCreditsSummary>,
    pub individual_limits: Vec<IndividualSpendLimit>,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub refreshing: bool,
    pub auth_url: Option<String>,
}

impl AccountSnapshot {
    pub(super) fn pending(record: &super::store::AccountRecord) -> Self {
        Self {
            id: record.id.clone(),
            provider: record.provider.clone(),
            label: record.label.clone(),
            origin: record.origin.clone(),
            identity: AccountIdentity::default(),
            plan: None,
            status: "pending".into(),
            observed_at: None,
            expires_at: None,
            retry_at: None,
            windows: vec![],
            balances: vec![],
            reset_credits: None,
            individual_limits: vec![],
            error: None,
            notice: None,
            refreshing: false,
            auth_url: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectAccountRequest {
    pub account_id: Option<String>,
    pub provider: String,
    pub method: String,
    pub label: Option<String>,
    pub path: Option<String>,
    pub api_key: Option<String>,
}

impl std::fmt::Debug for ConnectAccountRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectAccountRequest")
            .field("provider", &self.provider)
            .field("method", &self.method)
            .field("credential_configured", &self.api_key.is_some())
            .finish_non_exhaustive()
    }
}
