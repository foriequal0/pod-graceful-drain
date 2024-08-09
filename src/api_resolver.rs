use k8s_openapi::NamespaceResourceScope;
use kube::{Api, Client, Config, Resource, ResourceExt};

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

    pub fn impersonate_as(
        &self,
        user: Option<String>,
        group: Option<Vec<String>>,
    ) -> kube::Result<Self> {
        let mut config = self.config.clone();
        config.auth_info.impersonate = user;
        config.auth_info.impersonate_groups = group;
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

    pub fn default_namespaced<K>(&self) -> Api<K>
    where
        K: Resource<Scope = NamespaceResourceScope>,
        K::DynamicType: Default,
    {
        if let Some(ns) = self.namespace.as_ref() {
            Api::namespaced(self.client.clone(), ns)
        } else {
            Api::default_namespaced(self.client.clone())
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