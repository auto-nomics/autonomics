use crate::Observation;

pub trait Dimension {
    fn name(&self) -> &'static str;
    /// Extract value of current dimension from observation; None = Unanchored。
    /// 必须是"已存储字段的纯函数"——rebuild 靠重放它恢复分区。
    fn extract(&self, observation: &Observation) -> Option<String>;
}

pub struct ObservationKindDimension;

impl Dimension for ObservationKindDimension {
    fn name(&self) -> &'static str {
        "observation_kind"
    }

    fn extract(&self, observation: &Observation) -> Option<String> {
        Some(observation.kind_label().to_string())
    }
}

pub struct NodeKindDimension;

impl Dimension for NodeKindDimension {
    fn name(&self) -> &'static str {
        "node_kind"
    }

    fn extract(&self, observation: &Observation) -> Option<String> {
        observation.node_kind.clone()
    }
}
