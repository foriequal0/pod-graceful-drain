use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use k8s_openapi::serde::{Deserialize, Serialize};
use k8s_openapi::{Metadata, NamespaceResourceScope, Resource};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetGroupBinding {
    pub metadata: ObjectMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<TargetGroupBindingSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<TargetGroupBindingStatus>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetGroupBindingSpec {
    #[serde(rename = "targetGroupARN")]
    pub target_group_arn: String,
    pub target_type: Option<TargetType>,
    pub service_ref: Option<ServiceReference>,
    // not needed for our scenario
    // pub networking: Option<TargetGroupBindingNetworking>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetType {
    Instance,
    Ip,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceReference {
    pub name: String,
    pub port: IntOrString,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetGroupBindingStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
}

impl Resource for TargetGroupBinding {
    const API_VERSION: &'static str = "elbv2.k8s.aws/v1beta1";
    const GROUP: &'static str = "elbv2.k8s.aws";
    const KIND: &'static str = "TargetGroupBinding";
    const VERSION: &'static str = "v1beta1";
    const URL_PATH_SEGMENT: &'static str = "targetgroupbindings";

    type Scope = NamespaceResourceScope;
}

impl Metadata for TargetGroupBinding {
    type Ty = ObjectMeta;

    fn metadata(&self) -> &Self::Ty {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Self::Ty {
        &mut self.metadata
    }
}
