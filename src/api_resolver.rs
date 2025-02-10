use axum::http::{HeaderName, HeaderValue};
use eyre::Result;
use k8s_openapi::api::authentication::v1::UserInfo;
use k8s_openapi::NamespaceResourceScope;
use kube::{Api, Client, Config, Resource, ResourceExt};
use std::str::FromStr;

#[derive(Clone)]
pub struct ApiResolver {
    pub client: Client,
    config: Config,

    /// For namespace isolated test.
    namespace: Option<String>,
}

impl ApiResolver {
    pub fn try_new(config: Config) -> kube::Result<Self> {
        let client = Client::try_from(config.clone())?;
        Ok(Self {
            client,
            config,
            namespace: None,
        })
    }

    pub fn try_new_within(config: Config, ns: &str) -> kube::Result<Self> {
        let client = Client::try_from(config.clone())?;
        Ok(Self {
            client,
            config,
            namespace: Some(String::from(ns)),
        })
    }

    pub fn impersonate_as(&self, user: &UserInfo) -> Result<Self> {
        let mut config = self.config.clone();
        config.auth_info.impersonate = user.username.clone();
        config.auth_info.impersonate_groups = user.groups.clone();
        if let Some(uid) = &user.uid {
            config.headers.push((
                HeaderName::from_static("impersonate-uid"),
                HeaderValue::from_str(uid.as_str())?,
            ))
        }
        if let Some(extras) = &user.extra {
            for (name, values) in extras.iter() {
                let encoded_name =
                    percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
                        .to_string();
                for value in values.iter() {
                    let encoded_value =
                        percent_encoding::utf8_percent_encode(value, percent_encoding::CONTROLS)
                            .to_string();
                    config.headers.push((
                        HeaderName::from_str(&format!("impersonate-extra-{encoded_name}"))?,
                        HeaderValue::from_str(&encoded_value)?,
                    ))
                }
            }
        }
        let client = Client::try_from(config.clone())?;

        Ok(Self {
            client,
            config,
            namespace: self.namespace.clone(),
        })
    }

    pub fn all<K>(&self) -> Api<K>
    where
        K: Resource<Scope = NamespaceResourceScope>,
        K::DynamicType: Default,
    {
        if let Some(ns) = self.namespace.as_ref() {
            Api::namespaced(self.client.clone(), ns)
        } else {
            Api::all(self.client.clone())
        }
    }

    pub fn api_for<K>(&self, res: &K) -> Api<K>
    where
        K: Resource<Scope = NamespaceResourceScope>,
        K::DynamicType: Default,
    {
        if let Some(ns) = res.namespace() {
            Api::namespaced(self.client.clone(), &ns)
        } else {
            Api::all(self.client.clone())
        }
    }
}
