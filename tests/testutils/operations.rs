use std::any::TypeId;
use std::fmt::Debug;

use eyre::Result;
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::serde::de::DeserializeOwned;
use k8s_openapi::serde::Serialize;
use k8s_openapi::{ClusterResourceScope, NamespaceResourceScope};
use kube::api::{ApiResource, DynamicObject, Patch, PatchParams, PostParams};
use kube::{Api, Resource, ResourceExt};

use crate::testutils::context::TestContext;
use crate::testutils::run_command::{run_command, CommandParams};

pub async fn kubectl(context: &TestContext, args: &[&str], stdin: Option<&[u8]>) {
    let kubectl = std::env::var("KUBECTL").unwrap_or("kubectl".to_owned());
    run_command(&CommandParams {
        command: &kubectl,
        config_args: &[
            "--kubeconfig",
            context.kubeconfig.path().to_str().unwrap(),
            "--namespace",
            &context.namespace,
        ],
        args,
        stdin: stdin.map(|x| x.to_vec()),
    })
    .await
    .unwrap()
}

#[macro_export]
macro_rules! kubectl {
    ($ctx:expr, [$($arg:tt)*]) => {
        $crate::testutils::operations::kubectl($ctx, &[$($arg)*], None).await
    };
}

pub async fn apply<K>(context: &TestContext, res: &K) -> Result<K>
where
    K: Resource + Serialize + Debug + Clone + DeserializeOwned,
    K::DynamicType: Default,
    K::Scope: 'static,
{
    let dyntype = ApiResource::erase::<K>(&Default::default());
    let type_id = TypeId::of::<K::Scope>();
    let api: Api<DynamicObject> = {
        if type_id == TypeId::of::<NamespaceResourceScope>() {
            Api::namespaced_with(
                context.api_resolver.client.clone(),
                &context.namespace,
                &dyntype,
            )
        } else if type_id == TypeId::of::<ClusterResourceScope>() {
            Api::all_with(context.api_resolver.client.clone(), &dyntype)
        } else {
            unimplemented!();
        }
    };

    let name = res.name_any();
    let created = if api.get(&name).await.is_ok() {
        api.patch(
            &res.name_any(),
            &PatchParams::default(),
            &Patch::Strategic(res.clone()),
        )
        .await?
    } else {
        let json = serde_json::to_string(&res)?;
        let object = serde_json::from_str(&json)?;
        api.create(&PostParams::default(), &object).await?
    };

    if type_id == TypeId::of::<ClusterResourceScope>() {
        context
            .cluster_resources
            .lock()
            .unwrap()
            .insert((dyntype, name));
    }

    Ok(created.try_parse()?)
}

#[macro_export]
macro_rules! apply_yaml {
    ($ctx:expr, $kind:ty, $yaml:expr $(,)?) => {
        {
            let yaml = format!($yaml);
            let res = ::serde_yaml::from_str::<$kind>(&yaml).unwrap();
            $crate::testutils::operations::apply($ctx, &res).await.unwrap()
        }
    };
    ($ctx:expr, $kind:ty, $yaml:expr, $($arg:tt)+) => {
        {
            let yaml = format!($yaml, $($arg)+);
            let res = ::serde_yaml::from_str::<$kind>(&yaml).unwrap();
            $crate::testutils::operations::apply($ctx, &res).await.unwrap()
        }
    };
}

/// If you run a kind cluster in the docker, you can connect to the host with `host.docker.internal`
/// If it were in the podman, then it would've been `host.containers.internal`
/// Moreover, rootless podman requires `--network=slirp4netns:allow_host_loopback=true` argument,
/// but the kind podman provider doesn't pass the argument to create a cluster.
/// Even if it does, windows podman doesn't handle that argument well for now.
/// So, this is a reliable solution.
pub async fn install_test_host_service(context: &TestContext) -> String {
    let local_ip = local_ip_address::local_ip().unwrap();
    // Headless service
    apply_yaml!(
        context,
        Service,
        r#"
metadata:
  name: test-host
spec:
  type: ExternalName
  externalName: {local_ip}
"#
    );

    format!("test-host.{}.svc", context.namespace)
}
