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
#[serde(default)]
pub struct Policies {
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
#[error("invalid policies, err: {}", .0)]
pub struct InvalidPolicies(String);

/// Used just for json parsing
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
struct Envelope {
    policies: Policies,
}

impl Policies {
    /// Create a new empty [Policies] struct
    pub fn new() -> Self {
        Policies {
            ..Default::default()
        }
    }

    /// Create a new [Policies] from a json string
    pub fn from_json(json: impl Into<String>) -> Result<Self, InvalidPolicies> {
        let envelope: Envelope =
            serde_json::from_str(&json.into()).map_err(|e| InvalidPolicies(e.to_string()))?;
        Ok(envelope.policies)
    }

    /// Create a new [Policies] from a json file
    pub fn from_file(json_file_path: impl Into<String>) -> Result<Self, InvalidPolicies> {
        Policies::from_json(
            read_to_string(json_file_path.into()).map_err(|e| InvalidPolicies(e.to_string()))?,
        )
    }

    /// Convert [Policies] to json string
    pub fn to_json(&self) -> Result<String, InvalidPolicies> {
        let envelope = Envelope {
            policies: self.clone(),
        };
        serde_json::to_string(&envelope).map_err(|e| InvalidPolicies(e.to_string()))
    }

    /// Add an inbound policy
    pub fn add_inbound(&mut self, policy: impl Borrow<Policy>) -> &mut Self {
        self.inbound.push(policy.borrow().to_owned());
        self
    }

    /// Add an outbound policy
    pub fn add_outbound(&mut self, policy: impl Borrow<Policy>) -> &mut Self {
        self.outbound.push(policy.borrow().to_owned());
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
    pub fn new(
        type_: impl Into<String>,
        config: Option<impl Into<String>>,
    ) -> Result<Self, InvalidPolicies> {
        Ok(Action {
            type_: type_.into(),
            config: config
                .map(|c| {
                    serde_json::from_str(&c.into()).map_err(|e| InvalidPolicies(e.to_string()))
                })
                .transpose()?,
        })
    }
}

// transform into the wire protocol format
impl From<Policies> for proto::Policies {
    fn from(o: Policies) -> Self {
        proto::Policies {
            inbound: o.inbound.into_iter().map(|p| p.into()).collect(),
            outbound: o.outbound.into_iter().map(|p| p.into()).collect(),
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
        let policies = Policies::from_json(POLICY_JSON).unwrap();
        assert_eq!(1, policies.inbound.len());
        assert_eq!(1, policies.outbound.len());
        let inbound = &policies.inbound[0];
        let outbound = &policies.outbound[0];

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
    fn test_builders() {
        let policies = Policies::from_json(POLICY_JSON).unwrap();
        let policies2 = Policies::new()
            .add_inbound(
                Policy::new("test_in")
                    .add_expression("req.Method == 'PUT'")
                    .add_action(Action::new("deny", None::<String>).unwrap()),
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
        assert_eq!(policies, policies2);
    }
}
