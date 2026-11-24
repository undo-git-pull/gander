//! fzf-style matching for the catalog (`GET /catalog?q=`). Uses nucleo, which
//! implements fzf's scoring and query syntax: space-separated terms must all
//! match, smart case.

use crate::db::CatalogTable;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

pub fn filter_catalog(tables: Vec<CatalogTable>, query: &str) -> Vec<CatalogTable> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut score = |hay: &str| pattern.score(Utf32Str::new(hay, &mut buf), &mut matcher);
    let mut out: Vec<(u32, CatalogTable)> = Vec::new();
    for mut t in tables {
        let table_score = score(&format!("{}.{}", t.schema, t.name));
        let mut cols: Vec<(u32, usize)> = Vec::new();
        for (i, c) in t.columns.iter().enumerate() {
            if let Some(s) = score(&format!("{}.{}", t.name, c.name)) {
                cols.push((s, i));
            }
        }
        let best_col = cols.iter().map(|(s, _)| *s).max();
        let best = table_score.into_iter().chain(best_col).max();
        let Some(best) = best else { continue };
        if table_score.is_none() {
            // Only columns matched: keep just those, best first.
            cols.sort_by(|a, b| b.0.cmp(&a.0));
            let keep: Vec<usize> = cols.iter().map(|(_, i)| *i).collect();
            let all = std::mem::take(&mut t.columns);
            t.columns = keep.into_iter().map(|i| all[i].clone()).collect();
        }
        out.push((best, t));
    }
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    out.into_iter().map(|(_, t)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::CatalogColumn;

    fn t(name: &str, cols: &[&str]) -> CatalogTable {
        CatalogTable {
            database: "db".into(),
            schema: "main".into(),
            name: name.into(),
            kind: "table".into(),
            columns: cols
                .iter()
                .map(|c| CatalogColumn {
                    name: c.to_string(),
                    ty: "INTEGER".into(),
                })
                .collect(),
        }
    }

    #[test]
    fn matches_table_and_column_terms() {
        let r = filter_catalog(
            vec![t("orders", &["id", "amount"]), t("users", &["id", "name"])],
            "ord am",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].name, "orders");
        assert_eq!(r[0].columns.len(), 1);
        assert_eq!(r[0].columns[0].name, "amount");
    }

    #[test]
    fn table_match_keeps_all_columns() {
        let r = filter_catalog(
            vec![t("orders", &["id", "amount"]), t("users", &["id"])],
            "users",
        );
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].columns.len(), 1);
    }
}
