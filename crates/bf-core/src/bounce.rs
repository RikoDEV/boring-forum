//! Permanent-bounce addresses from a mail provider's webhook body.
//!
//! Understands the plain `{"email": ...}` form, SendGrid, Postmark, Mailgun and Amazon SES
//! over SNS. Temporary failures are ignored: only permanent ones count against an address.

use serde_json::Value;

fn one(v: &Value, out: &mut Vec<String>) {
    let s = |k: &str| v.get(k).and_then(Value::as_str);
    // SES through SNS: the event is a JSON string inside `Message`.
    if let Some(msg) = s("Message").and_then(|m| serde_json::from_str::<Value>(m).ok()) {
        return one(&msg, out);
    }
    if let Some(b) = v.get("bounce") {
        if b.get("bounceType").and_then(Value::as_str) == Some("Permanent") {
            let list = b.get("bouncedRecipients").and_then(Value::as_array);
            out.extend(list.into_iter().flatten().filter_map(|r| r["emailAddress"].as_str()).map(str::to_owned));
        }
        return;
    }
    if let Some(d) = v.get("event-data") {
        if d["event"] == "failed" && d["severity"] == "permanent" {
            out.extend(d["recipient"].as_str().map(str::to_owned));
        }
        return;
    }
    if let Some(kind) = s("RecordType") {
        if kind == "Bounce" && s("Type") == Some("HardBounce") {
            out.extend(s("Email").map(str::to_owned));
        }
        return;
    }
    if let Some(event) = s("event") {
        if event == "bounce" && s("type") != Some("blocked") {
            out.extend(s("email").map(str::to_owned));
        }
        return;
    }
    out.extend(s("email").map(str::to_owned));
}

/// The address to confirm an SNS subscription at, only if it really is an AWS SNS endpoint:
/// the server fetches it, so anything else would let a caller aim requests elsewhere.
pub fn sns_confirm_url(body: &Value) -> Option<&str> {
    if body.get("Type")?.as_str()? != "SubscriptionConfirmation" {
        return None;
    }
    let url = body.get("SubscribeURL")?.as_str()?;
    let host = url.strip_prefix("https://")?.split(['/', '?']).next()?;
    let ok = host.starts_with("sns.")
        && host.ends_with(".amazonaws.com")
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    ok.then_some(url)
}

pub fn permanent_addresses(body: &Value) -> Vec<String> {
    let mut out = Vec::new();
    match body {
        Value::Array(items) => items.iter().for_each(|v| one(v, &mut out)),
        v => one(v, &mut out),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sns_confirmation_only_goes_to_aws() {
        let c = |u: &str| json!({"Type":"SubscriptionConfirmation","SubscribeURL":u});
        let good = "https://sns.eu-west-1.amazonaws.com/?Action=ConfirmSubscription";
        assert_eq!(sns_confirm_url(&c(good)), Some(good));
        for bad in [
            "http://sns.eu-west-1.amazonaws.com/",
            "https://evil.example/",
            "https://sns.x.amazonaws.com@evil.example/",
            "https://sns.amazonaws.com.evil.example/",
        ] {
            assert_eq!(sns_confirm_url(&c(bad)), None, "{bad}");
        }
    }

    #[test]
    fn plain() {
        assert_eq!(permanent_addresses(&json!({"email": "a@x"})), ["a@x"]);
    }

    #[test]
    fn sendgrid() {
        let b = json!([{"event":"bounce","email":"a@x"},{"event":"delivered","email":"b@x"},{"event":"bounce","type":"blocked","email":"c@x"}]);
        assert_eq!(permanent_addresses(&b), ["a@x"]);
    }

    #[test]
    fn postmark() {
        assert_eq!(permanent_addresses(&json!({"RecordType":"Bounce","Type":"HardBounce","Email":"a@x"})), ["a@x"]);
        assert!(permanent_addresses(&json!({"RecordType":"Bounce","Type":"SoftBounce","Email":"a@x"})).is_empty());
    }

    #[test]
    fn mailgun() {
        let e = |sev| json!({"event-data":{"event":"failed","severity":sev,"recipient":"a@x"}});
        assert_eq!(permanent_addresses(&e("permanent")), ["a@x"]);
        assert!(permanent_addresses(&e("temporary")).is_empty());
    }

    #[test]
    fn ses_over_sns() {
        let m = |t: &str| json!({"Type":"Notification","Message":json!({"bounce":{"bounceType":t,"bouncedRecipients":[{"emailAddress":"a@x"}]}}).to_string()});
        assert_eq!(permanent_addresses(&m("Permanent")), ["a@x"]);
        assert!(permanent_addresses(&m("Transient")).is_empty());
    }
}
