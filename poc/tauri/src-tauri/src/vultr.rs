//! Vultr の API（試作。キーの要らない GET /v2/regions だけを呼ぶ）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const API: &str = "https://api.vultr.com/v2";

#[derive(Deserialize)]
struct RegionsResponse {
    regions: Vec<Region>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct Region {
    pub id: String,
    pub city: String,
    pub country: String,
    pub continent: String,
}

#[derive(Serialize)]
pub struct Country {
    /// ISO 3166-1 alpha-2（例: JP）
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

pub async fn countries() -> Result<Vec<Country>, String> {
    let res = reqwest::Client::new()
        .get(format!("{API}/regions?per_page=500"))
        .send()
        .await
        .map_err(|e| format!("Vultr の API につながりません: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("Vultr の API が失敗: HTTP {}", res.status()));
    }
    let body: RegionsResponse = res
        .json()
        .await
        .map_err(|e| format!("Vultr の API の応答を読めません: {e}"))?;
    Ok(group_by_country(body.regions))
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
        assert_eq!(c[1].regions.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["ewr", "sea"]);
    }
}
