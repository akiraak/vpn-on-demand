//! Vultr API v2 client.

use crate::error::{Result, VpnError};
use crate::vpn::{Cloud, Server};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

const API: &str = "https://api.vultr.com/v2";
/// Every server this app creates carries this tag. The app never touches servers without it.
pub const TAG: &str = "vpn-on-demand";
const OS_NAME: &str = "Ubuntu 24.04 LTS x64";
const PREFERRED_PLAN: &str = "vc2-1c-1gb";

// ---------- regions (no API key needed) ----------

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct Region {
    pub id: String,
    pub city: String,
    pub country: String,
    pub continent: String,
}

#[derive(Serialize, Debug)]
pub struct Country {
    /// ISO 3166-1 alpha-2 (e.g. JP)
    pub code: String,
    pub continent: String,
    pub regions: Vec<Region>,
}

pub fn group_by_country(regions: Vec<Region>) -> Vec<Country> {
    let mut map: BTreeMap<String, Country> = BTreeMap::new();
    for r in regions {
        map.entry(r.country.clone())
            .or_insert_with(|| Country {
                code: r.country.clone(),
                continent: r.continent.clone(),
                regions: Vec::new(),
            })
            .regions
            .push(r);
    }
    let mut countries: Vec<Country> = map.into_values().collect();
    for c in &mut countries {
        c.regions.sort_by(|a, b| a.city.cmp(&b.city));
    }
    countries
}

// ---------- plans ----------

#[derive(Deserialize, Clone, Debug)]
pub struct Plan {
    pub id: String,
    #[serde(default)]
    pub hourly_cost: f64,
    #[serde(default)]
    pub location_cost: HashMap<String, LocationCost>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LocationCost {
    #[serde(default)]
    pub hourly_cost: f64,
}

impl Plan {
    pub fn hourly_cost_in(&self, region: &str) -> f64 {
        self.location_cost
            .get(region)
            .map(|c| c.hourly_cost)
            .unwrap_or(self.hourly_cost)
    }
}

/// The cheapest vc2 plan with IPv4 that is available in the region. Prefers vc2-1c-1gb
/// (what the prototype used) when it is available and not more expensive.
pub fn pick_plan<'a>(plans: &'a [Plan], available: &[String], region: &str) -> Option<&'a Plan> {
    let mut candidates: Vec<&Plan> = plans
        .iter()
        .filter(|p| p.id.starts_with("vc2-"))
        .filter(|p| !p.id.ends_with("-v6") && !p.id.ends_with("-free"))
        .filter(|p| available.iter().any(|a| a == &p.id))
        .collect();
    candidates.sort_by(|a, b| {
        a.hourly_cost_in(region)
            .partial_cmp(&b.hourly_cost_in(region))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| (b.id == PREFERRED_PLAN).cmp(&(a.id == PREFERRED_PLAN)))
    });
    candidates.into_iter().next()
}

// ---------- client ----------

pub struct Vultr {
    http: reqwest::Client,
    key: String,
    plans: Mutex<Option<Vec<Plan>>>,
    /// How many times to retry when the network is unreachable, and how long to wait between.
    retries: u32,
    retry_delay: Duration,
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        // The route changes when the exit node is switched; don't reuse old connections.
        .pool_max_idle_per_host(0)
        .build()
        .expect("HTTP client")
}

pub async fn regions() -> Result<Vec<Region>> {
    #[derive(Deserialize)]
    struct R {
        regions: Vec<Region>,
    }
    let res = http_client()
        .get(format!("{API}/regions?per_page=500"))
        .send()
        .await
        .map_err(|e| VpnError::Cloud(format!("Can't reach the Vultr API: {e}")))?;
    if !res.status().is_success() {
        return Err(VpnError::Cloud(format!(
            "The Vultr API returned HTTP {}.",
            res.status().as_u16()
        )));
    }
    let r: R = res
        .json()
        .await
        .map_err(|e| VpnError::Cloud(format!("Unexpected response from Vultr: {e}")))?;
    Ok(r.regions)
}

fn describe_http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| body.chars().take(300).collect());
    match status {
        401 | 403 => format!(
            "Vultr rejected the API key (HTTP {status}). Check the key in Settings, and that \
             Access Control on Vultr's API page allows this PC's IP address. While connected, \
             your traffic leaves from the server's IP address, so \"Allow all IPv4\" and \
             \"Allow all IPv6\" are the simplest choice. ({detail})"
        ),
        _ => format!("The Vultr API returned HTTP {status}: {detail}"),
    }
}

impl Vultr {
    pub fn new(key: String) -> Result<Self> {
        if key.trim().is_empty() {
            return Err(VpnError::Setup(
                "The Vultr API key is not set. Add it in Settings.".into(),
            ));
        }
        Ok(Self {
            http: http_client(),
            key: key.trim().to_string(),
            plans: Mutex::new(None),
            retries: 8,
            retry_delay: Duration::from_secs(4),
        })
    }

    /// Sends a request. Retries while the network is unreachable (e.g. for a few seconds right
    /// after the exit node is switched off). Returns `None` for HTTP 404.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Option<Value>> {
        let mut attempt = 0;
        loop {
            let mut req = self
                .http
                .request(method.clone(), format!("{API}{path}"))
                .bearer_auth(&self.key);
            if let Some(b) = &body {
                req = req.json(b);
            }
            match req.send().await {
                Ok(res) => {
                    let status = res.status().as_u16();
                    let text = res.text().await.unwrap_or_default();
                    if status == 404 {
                        return Ok(None);
                    }
                    if !(200..300).contains(&status) {
                        return Err(VpnError::Cloud(describe_http_error(status, &text)));
                    }
                    if text.trim().is_empty() {
                        return Ok(Some(Value::Null));
                    }
                    return serde_json::from_str(&text).map(Some).map_err(|e| {
                        VpnError::Cloud(format!("Unexpected response from Vultr: {e}"))
                    });
                }
                Err(e)
                    if attempt < self.retries
                        && (e.is_connect() || e.is_timeout() || e.is_request()) =>
                {
                    attempt += 1;
                    tokio::time::sleep(self.retry_delay).await;
                }
                Err(e) => {
                    return Err(VpnError::Cloud(format!("Can't reach the Vultr API: {e}")));
                }
            }
        }
    }

    async fn get(&self, path: &str) -> Result<Value> {
        self.call(reqwest::Method::GET, path, None)
            .await?
            .ok_or_else(|| VpnError::Cloud(format!("Vultr: {path} not found")))
    }

    /// Checks the key. Returns a short, non-sensitive summary.
    pub async fn check(&self) -> Result<String> {
        let v = self.get("/account").await?;
        let a = v.get("account").cloned().unwrap_or(Value::Null);
        let balance = a.get("balance").and_then(Value::as_f64).unwrap_or(0.0);
        let pending = a
            .get("pending_charges")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        // Vultr shows credit as a negative balance.
        Ok(format!(
            "The API key works. Balance: ${:.2}, pending charges: ${pending:.2}.",
            -balance
        ))
    }

    /// The raw `GET /instances/{id}` object (`status`, `power_status`, `server_status`, ...), or
    /// `None` if it no longer exists. Used by the timing tool.
    pub async fn instance(&self, id: &str) -> Result<Option<Value>> {
        Ok(self
            .call(reqwest::Method::GET, &format!("/instances/{id}"), None)
            .await?
            .and_then(|v| v.get("instance").cloned()))
    }

    async fn plans(&self) -> Result<Vec<Plan>> {
        if let Some(p) = self.plans.lock().unwrap().clone() {
            return Ok(p);
        }
        let v = self.get("/plans?type=vc2&per_page=500").await?;
        let plans: Vec<Plan> = serde_json::from_value(v.get("plans").cloned().unwrap_or(json!([])))
            .map_err(|e| VpnError::Cloud(format!("Unexpected plan list from Vultr: {e}")))?;
        *self.plans.lock().unwrap() = Some(plans.clone());
        Ok(plans)
    }

    async fn os_id(&self) -> Result<i64> {
        let v = self.get("/os?per_page=500").await?;
        v.get("os")
            .and_then(Value::as_array)
            .and_then(|list| {
                list.iter()
                    .find(|o| o.get("name").and_then(Value::as_str) == Some(OS_NAME))
            })
            .and_then(|o| o.get("id").and_then(Value::as_i64))
            .ok_or_else(|| VpnError::Cloud(format!("Vultr no longer offers \"{OS_NAME}\".")))
    }
}

fn to_server(v: &Value) -> Server {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Server {
        id: s("id"),
        label: s("label"),
        region: s("region"),
        plan: s("plan"),
        main_ip: s("main_ip"),
        status: s("status"),
        created: s("date_created"),
        tags: v
            .get("tags")
            .and_then(Value::as_array)
            .map(|t| {
                t.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    }
}

impl Cloud for Vultr {
    async fn list_servers(&self) -> Result<Vec<Server>> {
        let v = self
            .get(&format!("/instances?tag={TAG}&per_page=500"))
            .await?;
        Ok(v.get("instances")
            .and_then(Value::as_array)
            .map(|list| list.iter().map(to_server).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            // Never trust the filter alone: only servers that really carry our tag.
            .filter(|s| s.tags.iter().any(|t| t == TAG))
            .collect())
    }

    async fn create_server(&self, region: &str, hostname: &str, user_data: &str) -> Result<Server> {
        let avail = self.get(&format!("/regions/{region}/availability")).await?;
        let available: Vec<String> = avail
            .get("available_plans")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let plans = self.plans().await?;
        let plan = pick_plan(&plans, &available, region).ok_or_else(|| {
            VpnError::Cloud(format!(
                "No small server plan is available in {region} right now. Try another city."
            ))
        })?;
        let body = json!({
            "region": region,
            "plan": plan.id,
            "os_id": self.os_id().await?,
            "label": hostname,
            "hostname": hostname,
            "tags": [TAG],
            "enable_ipv6": true,
            "backups": "disabled",
            "activation_email": false,
            "user_data": user_data,
        });
        let v = self
            .call(reqwest::Method::POST, "/instances", Some(body))
            .await?
            .ok_or_else(|| VpnError::Cloud("Vultr: creating the server failed (404).".into()))?;
        Ok(to_server(v.get("instance").unwrap_or(&Value::Null)))
    }

    async fn delete_server(&self, id: &str) -> Result<()> {
        // 404 means it is already gone, which is what we want.
        self.call(reqwest::Method::DELETE, &format!("/instances/{id}"), None)
            .await
            .map(|_| ())
    }

    async fn hourly_cost(&self, plan: &str, region: &str) -> Option<f64> {
        let plans = self.plans().await.ok()?;
        plans
            .iter()
            .find(|p| p.id == plan)
            .map(|p| p.hourly_cost_in(region))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, city: &str, country: &str) -> Region {
        Region {
            id: id.into(),
            city: city.into(),
            country: country.into(),
            continent: "X".into(),
        }
    }

    #[test]
    fn groups_regions_by_country() {
        let c = group_by_country(vec![
            r("sea", "Seattle", "US"),
            r("nrt", "Tokyo", "JP"),
            r("ewr", "New Jersey", "US"),
            r("itm", "Osaka", "JP"),
        ]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].code, "JP");
        assert_eq!(c[0].regions[0].id, "itm");
        assert_eq!(
            c[1].regions
                .iter()
                .map(|r| r.id.as_str())
                .collect::<Vec<_>>(),
            ["ewr", "sea"]
        );
    }

    fn plan(id: &str, cost: f64, loc: &[(&str, f64)]) -> Plan {
        Plan {
            id: id.into(),
            hourly_cost: cost,
            location_cost: loc
                .iter()
                .map(|(r, c)| (r.to_string(), LocationCost { hourly_cost: *c }))
                .collect(),
        }
    }

    #[test]
    fn picks_cheapest_available_ipv4_plan() {
        let plans = vec![
            plan("vc2-1c-0.5gb-free", 0.0, &[]),
            plan("vc2-1c-0.5gb-v6", 0.003, &[]),
            plan("vc2-1c-0.5gb", 0.005, &[]),
            plan("vc2-1c-1gb", 0.007, &[("sao", 0.01)]),
            plan("vc2-1c-2gb", 0.015, &[]),
            plan("vhf-1c-1gb", 0.008, &[]),
        ];
        let avail = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Tokyo: 0.5gb isn't offered there.
        let a = avail(&["vc2-1c-1gb", "vc2-1c-2gb", "vhf-1c-1gb", "vc2-1c-0.5gb-v6"]);
        assert_eq!(pick_plan(&plans, &a, "nrt").unwrap().id, "vc2-1c-1gb");
        // Where 0.5gb with IPv4 is offered, it is cheaper.
        let a = avail(&["vc2-1c-0.5gb", "vc2-1c-1gb", "vc2-1c-0.5gb-free"]);
        assert_eq!(pick_plan(&plans, &a, "ewr").unwrap().id, "vc2-1c-0.5gb");
        // Location cost is used.
        assert_eq!(plans[3].hourly_cost_in("sao"), 0.01);
        assert_eq!(plans[3].hourly_cost_in("nrt"), 0.007);
        // Nothing available.
        assert!(pick_plan(&plans, &avail(&["vhf-1c-1gb"]), "nrt").is_none());
    }

    #[test]
    fn explains_rejected_key() {
        let m = describe_http_error(401, r#"{"error":"Invalid API token.","status":401}"#);
        assert!(m.contains("Access Control"));
        assert!(m.contains("Invalid API token."));
        let m = describe_http_error(400, r#"{"error":"Out of stock","status":400}"#);
        assert_eq!(m, "The Vultr API returned HTTP 400: Out of stock");
    }
}
