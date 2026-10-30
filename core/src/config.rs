//! What the hunt goes by. Nothing to set: what's caught (a reskin and a gradient, any
//! Pokemon), the catch plan and the rest are built in. config.json keeps only the
//! Discord webhook (the pill's box, the phone app's); anything else in an old one is
//! passed over. with_rules reads rules too, for the tests.

use crate::rules::{show, Rule, FIELDS};
use serde_json::{json, Map, Value};

#[derive(Clone, Debug)]
pub struct Config {
    pub raw: Map<String, Value>,
    pub webhook: String,
    pub ping: String,
    pub targets: Vec<Rule>,
    /// Worth catching (every reskin with a gradient too, unless "catch_reskin_gradient": false).
    pub notify: Vec<Rule>,
    /// Worth a Discord post, not a ball.
    pub post: Vec<Rule>,
    /// The catch plan: CATCH_PLAN with config.json's "catch" over it.
    pub catch: Map<String, Value>,
}

/// Caught unless "catch_reskin_gradient": false.
pub fn auto_catch() -> Rule {
    json!({"reskin": "any", "gradient": "any"}).as_object().unwrap().clone()
}

/// config.json "catch" overrides these.
pub fn catch_plan() -> Map<String, Value> {
    json!({
        "first_ball": "Quick Ball", "sleep_move": "Yawn", "guard_move": "Protect",
        "asleep_ball": "Repeat Ball", "awake_ball": "Repeat Ball", "new_ball": "Timer Ball",
        // a favorited Level Ball (x8) goes before every other ball; "" for none
        "level_ball": "Level Ball",
        "max_balls": 6767,
        // x1 + 0.3 for each turn passed (x4 at most): past a Repeat Ball's x3.5 once 9 turns have gone by
        "timer_ball": "Timer Ball", "timer_after": 9,
        "max_turns": 6767, "legendary_max_balls": 6767, "legendary_max_turns": 6767,
        "heal_below": 0.4, "heal_item": "Full Restore",
        "pp_item": "Max Elixir", "on_fail": "hold",
        "sleep_with": "Clodsire"  // the lead that knows Yawn and Protect; others just throw balls
    })
    .as_object()
    .unwrap()
    .clone()
}

pub const WEB_BALLS: [&str; 3] = ["Quick Ball", "Timer Ball", "Repeat Ball"]; // the page's Bag, unless "web_balls" says

impl Config {
    /// config.json: its webhook and nothing else, the rest built in.
    /// Never wrong: a file that isn't JSON has no webhook.
    pub fn parse(text: &str) -> Result<Config, String> {
        let raw: Map<String, Value> = serde_json::from_str(text).unwrap_or_default();
        let mut kept = Map::new();
        if let Some(v) = raw.get("webhook") {
            kept.insert("webhook".to_string(), v.clone());
        }
        Config::with_rules(&Value::Object(kept).to_string())
    }

    /// A config with rules and settings of its own (the tests'), or what's wrong with it.
    pub fn with_rules(text: &str) -> Result<Config, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("config.json: {e}"))?;
        let Value::Object(raw) = v else { return Err("config.json: not a {...}".into()) };
        let rules = |key: &str| -> Result<Vec<Rule>, String> {
            match raw.get(key) {
                None | Some(Value::Null) => Ok(Vec::new()),
                Some(Value::Array(a)) => a.iter().map(|r| r.as_object().cloned()
                    .ok_or_else(|| format!("config.json: {key} has a rule that isn't a {{...}}"))).collect(),
                Some(_) => Err(format!("config.json: {key} isn't a list")),
            }
        };
        let targets = rules("targets")?;
        let mut notify = rules("notify")?;
        // a legendary with just a reskin, or just a gradient, isn't caught: the bar is a reskin
        // and a gradient, for every Pokemon (auto_catch). Those two rules were in the example
        // config every install started from, so they're dropped here too
        let half = [json!({"species": "legendary", "reskin": "any"}), json!({"species": "legendary", "gradient": "any"})];
        notify.retain(|r| !half.iter().any(|h| h.as_object() == Some(r)));
        let post = rules("post")?;
        for rule in targets.iter().chain(&notify).chain(&post) {
            let mut bad: Vec<&String> =
                rule.keys().filter(|k| !FIELDS.contains(&k.as_str()) && k.as_str() != "not_species").collect();
            bad.sort();
            if !bad.is_empty() {
                return Err(format!("config.json: unknown field(s) {} in {}; use {}",
                                   bad.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "), show(rule),
                                   FIELDS.join(", ")));
            }
        }
        if raw.get("catch_reskin_gradient").and_then(Value::as_bool).unwrap_or(true) && !notify.contains(&auto_catch()) {
            notify.push(auto_catch());
        }
        if targets.is_empty() && notify.is_empty() {
            return Err("config.json: nothing to hunt for (no targets, and nothing to catch)".into());
        }
        let mut catch = catch_plan();
        if let Some(Value::Object(c)) = raw.get("catch") {
            for (k, v) in c {
                catch.insert(k.clone(), v.clone());
            }
        }
        let text = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Ok(Config {
            webhook: text("webhook"),
            ping: text("ping"),
            targets,
            notify,
            post,
            catch,
            raw,
        })
    }

    pub fn load(path: &std::path::Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Config::parse(&text)
    }

    /// A catch plan setting, as text ("" if it's off or missing).
    pub fn str(&self, key: &str) -> String {
        match self.catch.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | Some(Value::Bool(false)) | None => String::new(),
            Some(v) => v.to_string(),
        }
    }

    pub fn num(&self, key: &str) -> f64 {
        self.catch.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }

    pub fn flag(&self, key: &str) -> bool {
        match self.catch.get(key) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Null) | None => false,
            Some(Value::String(s)) => !s.is_empty(),
            Some(Value::Number(n)) => n.as_f64() != Some(0.0),
            Some(_) => true,
        }
    }

    /// A top-level setting: a number, or the default.
    pub fn number(&self, key: &str, default: f64) -> f64 {
        self.raw.get(key).and_then(Value::as_f64).unwrap_or(default)
    }

    pub fn boolean(&self, key: &str, default: bool) -> bool {
        self.raw.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn web_balls(&self) -> Vec<String> {
        self.raw.get("web_balls").and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|b| b.as_str().map(String::from)).collect())
            .unwrap_or_else(|| WEB_BALLS.iter().map(|s| s.to_string()).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_webhook_is_read() {
        let c = Config::parse(r#"{"webhook": "https://x", "targets": [{"species": "Mew"}], "catch": {"max_balls": 2}}"#).unwrap();
        assert_eq!(c.webhook, "https://x");
        assert!(c.targets.is_empty());
        assert_eq!(c.notify, vec![auto_catch()]);
        assert_eq!(c.num("max_balls"), 6767.0);
        assert_eq!(Config::parse("not json").unwrap().webhook, "");
    }

    #[test]
    fn an_empty_config_catches_reskins_with_gradients() {
        let c = Config::parse(r#"{"webhook": "", "targets": [], "notify": []}"#).unwrap();
        assert_eq!(c.notify, vec![auto_catch()]);
        assert_eq!(c.str("first_ball"), "Quick Ball");
        assert_eq!(c.num("max_balls"), 6767.0);
        assert_eq!(c.num("max_turns"), 6767.0);
        assert_eq!(c.num("legendary_max_balls"), 6767.0);
        assert_eq!(c.num("legendary_max_turns"), 6767.0);
    }

    #[test]
    fn mistakes_are_said() {
        assert!(Config::with_rules(r#"{"targets": [{"specie": "Mew"}]}"#).unwrap_err().contains("unknown field(s) specie"));
        assert!(Config::with_rules(r#"{"catch_reskin_gradient": false}"#).unwrap_err().contains("nothing to hunt for"));
        // a legendary needs a reskin and a gradient too, as every Pokemon
        let c = Config::with_rules(r#"{"notify": [{"species": "legendary", "reskin": "any"},
                                             {"species": "legendary", "gradient": "any"},
                                             {"species": "legendary", "reskin": "Night"}]}"#).unwrap();
        assert_eq!(c.notify.len(), 2); // the Night one, and reskin + gradient
        let c = Config::with_rules(r#"{"targets": [{"species": "Mew"}], "catch": {"first_ball": null}}"#).unwrap();
        assert_eq!(c.str("first_ball"), "");
    }
}
