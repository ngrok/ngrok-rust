use std::{
    borrow::Borrow,
    fs::read_to_string,
};

use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

use crate::internals::proto;

/// A set of policies that define rules that should be applied to incoming or outgoing
/// connections to the edge.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct Policies {
    policies: PolicySet,
}

/// A private layer to hold the inbound and outbound policies
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
struct PolicySet {
    inbound: Vec<Policy>,
    outbound: Vec<Policy>,
}

/// A policy that defines rules that should be applied
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Policy {
    name: String,
    expressions: Vec<String>,
    actions: Vec<Action>,
}

/// An action that should be taken if the policy matches
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Action {
    #[serde(rename = "type")]
    type_: String,
    config: Option<serde_json::Value>,
}

/// Error representing invalid string for Policies
#[derive(Debug, Clone, Error)]
pub enum InvalidPolicies {
    /// Error representing invalid string for Policies
    #[error("failure to parse policies, err: {}", .0)]
    ParseError(String),
    /// Error representing invalid string for Policies
    #[error("failure to serialize policies, err: {}", .0)]
    GenerationError(String),
    /// Error representing invalid string for Policies
    #[error("failure to read policies file '{}', err: {}", .0, .1)]
    FileReadError(String, String),
}

impl Policies {
    /// Create a new empty [Policies] struct
    pub fn new() -> Self {
        Policies {
            ..Default::default()
        }
    }

    /// Create a new [Policies] from a json string
    pub fn from_json(json: impl AsRef<str>) -> Result<Self, InvalidPolicies> {
        serde_json::from_str(json.as_ref()).map_err(|e| InvalidPolicies::ParseError(e.to_string()))
    }

    /// Create a new [Policies] from a json file
    pub fn from_file(json_file_path: impl AsRef<str>) -> Result<Self, InvalidPolicies> {
        Policies::from_json(read_to_string(json_file_path.as_ref()).map_err(|e| {
            InvalidPolicies::FileReadError(json_file_path.as_ref().to_string(), e.to_string())
        })?)
    }

    /// Convert [Policies] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicies> {
        serde_json::to_string(&self).map_err(|e| InvalidPolicies::GenerationError(e.to_string()))
    }

    /// Add an inbound policy
    pub fn add_inbound(&mut self, policy: impl Borrow<Policy>) -> &mut Self {
        self.policies.inbound.push(policy.borrow().to_owned());
        self
    }

    /// Add an outbound policy
    pub fn add_outbound(&mut self, policy: impl Borrow<Policy>) -> &mut Self {
        self.policies.outbound.push(policy.borrow().to_owned());
        self
    }
}

impl Policy {
    /// Create a new [Policy]
    pub fn new(name: impl Into<String>) -> Self {
        Policy {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Convert [Policy] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicies> {
        serde_json::to_string(&self).map_err(|e| InvalidPolicies::GenerationError(e.to_string()))
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

impl Action {
    /// Create a new [Action]
    pub fn new(type_: impl Into<String>, config: Option<&str>) -> Result<Self, InvalidPolicies> {
        Ok(Action {
            type_: type_.into(),
            config: config
                .map(|c| {
                    serde_json::from_str(c).map_err(|e| InvalidPolicies::ParseError(e.to_string()))
                })
                .transpose()?,
        })
    }

    /// Convert [Action] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicies> {
        serde_json::to_string(&self).map_err(|e| InvalidPolicies::GenerationError(e.to_string()))
    }
}

// transform into the wire protocol format
impl From<Policies> for proto::Policies {
    fn from(o: Policies) -> Self {
        proto::Policies {
            inbound: o.policies.inbound.into_iter().map(|p| p.into()).collect(),
            outbound: o.policies.outbound.into_iter().map(|p| p.into()).collect(),
        }
    }
}

impl From<Policy> for proto::Policy {
    fn from(p: Policy) -> Self {
        proto::Policy {
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
        {"policies":
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
        }
        "###;

    #[test]
    fn test_json_to_policies() {
        let pol = Policies::from_json(POLICY_JSON).unwrap();
        assert_eq!(1, pol.policies.inbound.len());
        assert_eq!(1, pol.policies.outbound.len());
        let inbound = &pol.policies.inbound[0];
        let outbound = &pol.policies.outbound[0];

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
    fn test_policies_to_json() {
        let policies = Policies::from_json(POLICY_JSON).unwrap();
        let json = policies.to_json().unwrap();
        let policies2 = Policies::from_json(json).unwrap();
        assert_eq!(policies, policies2);
    }

    #[test]
    fn test_policies_to_json_error() {
        let error = Policies::from_json("asdf").err().unwrap();
        assert!(matches!(error, InvalidPolicies::ParseError { .. }));
    }

    #[test]
    fn test_policy_to_json() {
        let pol = Policies::from_json(POLICY_JSON).unwrap();
        let policy = &pol.policies.outbound[0];
        let json = policy.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let policy_map = parsed.as_object().unwrap();
        assert_eq!("test_out", policy_map["name"]);

        // expressions
        let expressions = policy_map["expressions"].as_array().unwrap();
        assert_eq!(1, expressions.len());
        assert_eq!("res.StatusCode == '200'", expressions[0]);

        // actions
        let actions = policy_map["actions"].as_array().unwrap();
        assert_eq!(1, actions.len());
        assert_eq!("custom-response", actions[0]["type"]);
        assert_eq!(201, actions[0]["config"]["status_code"]);
    }

    #[test]
    fn test_action_to_json() {
        let pol = Policies::from_json(POLICY_JSON).unwrap();
        let action = &pol.policies.outbound[0].actions[0];
        let json = action.to_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let action_map = parsed.as_object().unwrap();
        assert_eq!("custom-response", action_map["type"]);
        assert_eq!(201, action_map["config"]["status_code"]);
    }

    #[test]
    fn test_builders() {
        let policies = Policies::from_json(POLICY_JSON).unwrap();
        let policies2 = Policies::new()
            .add_inbound(
                Policy::new("test_in")
                    .add_expression("req.Method == 'PUT'")
                    .add_action(Action::new("deny", None).unwrap()),
            )
            .add_outbound(
                Policy::new("test_out")
                    .add_expression("res.StatusCode == '200'")
                    // .add_action(Action::new("deny", ""))
                    .add_action(
                        Action::new("custom-response", Some("{\"status_code\":201}")).unwrap(),
                    ),
            )
            .to_owned();
        assert_eq!(policies, policies2);
    }

    #[test]
    fn test_load_file() {
        let policies = Policies::from_json(POLICY_JSON).unwrap();
        let policies2 = Policies::from_file("assets/policies.json").unwrap();
        assert_eq!("test_in", policies2.policies.inbound[0].name);
        assert_eq!("test_out", policies2.policies.outbound[0].name);
        assert_eq!(policies, policies2);
    }

    #[test]
    fn test_load_file_error() {
        let error = Policies::from_file("assets/absent.json").err().unwrap();
        assert!(matches!(error, InvalidPolicies::FileReadError { .. }));
    }
}
