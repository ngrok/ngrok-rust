use std::{
    fs::read_to_string,
    io,
};

use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

use crate::internals::proto;

/// A policy that defines rules that should be applied to incoming or outgoing
/// connections to the edge.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Policy {
    inbound: Vec<Rule>,
    outbound: Vec<Rule>,
}

/// A policy rule that should be applied
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Rule {
    name: String,
    expressions: Vec<String>,
    actions: Vec<Action>,
}

/// An action that should be taken if the rule matches
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Action {
    #[serde(rename = "type")]
    type_: String,
    config: Option<serde_json::Value>,
}

/// Errors in creating or serializing Policies
#[derive(Debug, Error)]
pub enum InvalidPolicy {
    /// Error representing an invalid string for a Policy
    #[error("failure to parse or generate policy")]
    SerializationError(#[from] serde_json::Error),
    /// An error loading a Policy from a file
    #[error("failure to read policy file '{}'", .1)]
    FileReadError(#[source] io::Error, String),
}

impl Policy {
    /// Create a new empty [Policy] struct
    pub fn new() -> Self {
        Policy {
            ..Default::default()
        }
    }

    /// Create a new [Policy] from a json string
    fn from_json(json: impl AsRef<str>) -> Result<Self, InvalidPolicy> {
        serde_json::from_str(json.as_ref()).map_err(InvalidPolicy::SerializationError)
    }

    /// Create a new [Policy] from a json file
    pub fn from_file(json_file_path: impl AsRef<str>) -> Result<Self, InvalidPolicy> {
        Policy::from_json(
            read_to_string(json_file_path.as_ref()).map_err(|e| {
                InvalidPolicy::FileReadError(e, json_file_path.as_ref().to_string())
            })?,
        )
    }

    /// Convert [Policy] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicy> {
        serde_json::to_string(&self).map_err(InvalidPolicy::SerializationError)
    }

    /// Add an inbound policy
    pub fn add_inbound(&mut self, rule: impl Into<Rule>) -> &mut Self {
        self.inbound.push(rule.into());
        self
    }

    /// Add an outbound policy
    pub fn add_outbound(&mut self, rule: impl Into<Rule>) -> &mut Self {
        self.outbound.push(rule.into());
        self
    }
}

impl TryFrom<&Policy> for Policy {
    type Error = InvalidPolicy;

    fn try_from(other: &Policy) -> Result<Policy, Self::Error> {
        Ok(other.clone())
    }
}

impl TryFrom<Result<Policy, InvalidPolicy>> for Policy {
    type Error = InvalidPolicy;

    fn try_from(other: Result<Policy, InvalidPolicy>) -> Result<Policy, Self::Error> {
        other
    }
}

impl TryFrom<&str> for Policy {
    type Error = InvalidPolicy;

    fn try_from(other: &str) -> Result<Policy, Self::Error> {
        Policy::from_json(other)
    }
}

impl Rule {
    /// Create a new [Rule]
    pub fn new(name: impl Into<String>) -> Self {
        Rule {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Convert [Rule] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicy> {
        serde_json::to_string(&self).map_err(InvalidPolicy::SerializationError)
    }

    /// Add an expression
    pub fn add_expression(&mut self, expression: impl Into<String>) -> &mut Self {
        self.expressions.push(expression.into());
        self
    }

    /// Add an action
    pub fn add_action(&mut self, action: Action) -> &mut Self {
        self.actions.push(action);
        self
    }
}

impl From<&mut Rule> for Rule {
    fn from(other: &mut Rule) -> Self {
        other.to_owned()
    }
}

impl Action {
    /// Create a new [Action]
    pub fn new(type_: impl Into<String>, config: Option<&str>) -> Result<Self, InvalidPolicy> {
        Ok(Action {
            type_: type_.into(),
            config: config
                .map(|c| serde_json::from_str(c).map_err(InvalidPolicy::SerializationError))
                .transpose()?,
        })
    }

    /// Convert [Action] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicy> {
        serde_json::to_string(&self).map_err(InvalidPolicy::SerializationError)
    }
}

// transform into the wire protocol format
impl From<Policy> for proto::Policy {
    fn from(o: Policy) -> Self {
        proto::Policy {
            inbound: o.inbound.into_iter().map(|p| p.into()).collect(),
            outbound: o.outbound.into_iter().map(|p| p.into()).collect(),
        }
    }
}

impl From<Rule> for proto::Rule {
    fn from(p: Rule) -> Self {
        proto::Rule {
            name: p.name,
            expressions: p.expressions,
            actions: p.actions.into_iter().map(|a| a.into()).collect(),
        }
    }
}

impl From<Action> for proto::Action {
    fn from(a: Action) -> Self {
        proto::Action {
            type_: a.type_,
            config: a
                .config
                .map(|c| c.to_string().into_bytes())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::*;

    pub(crate) const POLICY_JSON: &str = r###"
        {"inbound": [
            {
                "name": "test_in",
                "expressions": ["req.Method == 'PUT'"],
                "actions": [{"type": "deny"}]
            }
        ],
        "outbound": [
            {
                "name": "test_out",
                "expressions": ["res.StatusCode == '200'"],
                "actions": [{"type": "custom-response", "config": {"status_code":201}}]
            }
        ]}
        "###;

    #[test]
    fn test_json_to_policy() {
        let policy: Policy = Policy::from_json(POLICY_JSON).unwrap();
        assert_eq!(1, policy.inbound.len());
        assert_eq!(1, policy.outbound.len());
        let inbound = &policy.inbound[0];
        let outbound = &policy.outbound[0];

        assert_eq!("test_in", inbound.name);
        assert_eq!(1, inbound.expressions.len());
        assert_eq!(1, inbound.actions.len());
        assert_eq!("req.Method == 'PUT'", inbound.expressions[0]);
        assert_eq!("deny", inbound.actions[0].type_);
        assert_eq!(None, inbound.actions[0].config);

        assert_eq!("test_out", outbound.name);
        assert_eq!(1, outbound.expressions.len());
        assert_eq!(1, outbound.actions.len());
        assert_eq!("res.StatusCode == '200'", outbound.expressions[0]);
        assert_eq!("custom-response", outbound.actions[0].type_);
        assert_eq!(
            "{\"status_code\":201}",
            outbound.actions[0].config.as_ref().unwrap().to_string()
        );
    }

    #[test]
    fn test_empty_json_to_policy() {
        let policy: Policy = Policy::from_json("{}").unwrap();
        assert_eq!(0, policy.inbound.len());
        assert_eq!(0, policy.outbound.len());
    }

    #[test]
    fn test_policy_to_json() {
        let policy = Policy::from_json(POLICY_JSON).unwrap();
        let json = policy.to_json().unwrap();
        let policy2 = Policy::from_json(json).unwrap();
        assert_eq!(policy, policy2);
    }

    #[test]
    fn test_policy_to_json_error() {
        let error = Policy::from_json("asdf").err().unwrap();
        assert!(matches!(error, InvalidPolicy::SerializationError { .. }));
    }

    #[test]
    fn test_rule_to_json() {
        let policy = Policy::from_json(POLICY_JSON).unwrap();
        let rule = &policy.outbound[0];
        let json = rule.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let rule_map = parsed.as_object().unwrap();
        assert_eq!("test_out", rule_map["name"]);

        // expressions
        let expressions = rule_map["expressions"].as_array().unwrap();
        assert_eq!(1, expressions.len());
        assert_eq!("res.StatusCode == '200'", expressions[0]);

        // actions
        let actions = rule_map["actions"].as_array().unwrap();
        assert_eq!(1, actions.len());
        assert_eq!("custom-response", actions[0]["type"]);
        assert_eq!(201, actions[0]["config"]["status_code"]);
    }

    #[test]
    fn test_action_to_json() {
        let policy = Policy::from_json(POLICY_JSON).unwrap();
        let action = &policy.outbound[0].actions[0];
        let json = action.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let action_map = parsed.as_object().unwrap();
        assert_eq!("custom-response", action_map["type"]);
        assert_eq!(201, action_map["config"]["status_code"]);
    }

    #[test]
    fn test_builders() {
        let policy = Policy::from_json(POLICY_JSON).unwrap();
        let policy2 = Policy::new()
            .add_inbound(
                Rule::new("test_in")
                    .add_expression("req.Method == 'PUT'")
                    .add_action(Action::new("deny", None).unwrap()),
            )
            .add_outbound(
                Rule::new("test_out")
                    .add_expression("res.StatusCode == '200'")
                    // .add_action(Action::new("deny", ""))
                    .add_action(
                        Action::new("custom-response", Some("{\"status_code\":201}")).unwrap(),
                    ),
            )
            .to_owned();
        assert_eq!(policy, policy2);
    }

    #[test]
    fn test_load_file() {
        let policy = Policy::from_json(POLICY_JSON).unwrap();
        let policy2 = Policy::from_file("assets/policy.json").unwrap();
        assert_eq!("test_in", policy2.inbound[0].name);
        assert_eq!("test_out", policy2.outbound[0].name);
        assert_eq!(policy, policy2);
    }

    #[test]
    fn test_load_inbound_file() {
        let policy = Policy::from_file("assets/policy-inbound.json").unwrap();
        assert_eq!("test_in", policy.inbound[0].name);
        assert_eq!(0, policy.outbound.len());
    }

    #[test]
    fn test_load_file_error() {
        let error = Policy::from_file("assets/absent.json").err().unwrap();
        assert!(matches!(error, InvalidPolicy::FileReadError { .. }));
    }
}
