use std::{error::Error, io};

use crate::{app_icon::favicon_data_uri, history::HistorySample};

use super::model::{build_data, DashboardData};

const SECONDS_PER_DAY: i64 = 86_400;
const DATA_TOKEN: &str = "__VOLTY_DATA__";
const FAVICON_TOKEN: &str = "__VOLTY_FAVICON__";

pub(super) fn render_dashboard(
    samples: Vec<HistorySample>,
    days: u32,
    generated_at: i64,
) -> Result<String, Box<dyn Error>> {
    let window_start = generated_at.saturating_sub(i64::from(days).saturating_mul(SECONDS_PER_DAY));
    let data = build_data(samples, days, generated_at, window_start)?;
    render_html(&data)
}

fn render_html(data: &DashboardData) -> Result<String, Box<dyn Error>> {
    let serialized = serde_json::to_string(data)?;
    let serialized = escape_script_end_tags(&serialized);
    let template = include_str!("../dashboard.html");
    for token in [DATA_TOKEN, FAVICON_TOKEN] {
        if template.matches(token).count() != 1 {
            return Err(io::Error::other(format!(
                "dashboard template must contain exactly one {token} token"
            ))
            .into());
        }
    }
    Ok(template
        .replacen(DATA_TOKEN, &serialized, 1)
        .replacen(FAVICON_TOKEN, favicon_data_uri(), 1))
}

fn escape_script_end_tags(json: &str) -> String {
    json.replace("</", "<\\/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_html_embeds_the_favicon_and_data() {
        let html = render_html(&DashboardData {
            generated_at: 1,
            window_days: 7,
            devices: Vec::new(),
        })
        .unwrap();

        assert!(html.contains("href=\"data:image/png;base64,"));
        assert!(html.contains("\"generated_at\":1"));
        assert!(!html.contains("__VOLTY_FAVICON__"));
        assert!(!html.contains("__VOLTY_DATA__"));
    }

    #[test]
    fn json_escaping_prevents_closing_the_script_element() {
        let json = serde_json::to_string(&"</script><b>unsafe</b>").unwrap();
        let escaped = escape_script_end_tags(&json);

        assert!(escaped.contains("<\\/script>"));
        assert_eq!(
            serde_json::from_str::<String>(&escaped).unwrap(),
            "</script><b>unsafe</b>"
        );
    }
}
