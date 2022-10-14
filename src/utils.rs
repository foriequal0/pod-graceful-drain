use eyre::eyre;
use eyre::Result;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::api::{DeleteParams, Preconditions, PropagationPolicy};
use kube::runtime::reflector::ObjectRef;
use kube::Resource;

pub fn get_object_ref_from_name<K: Resource>(
    name: impl AsRef<str>,
    ns: Option<impl AsRef<str>>,
) -> ObjectRef<K>
where
    K::DynamicType: Default,
{
    let object_ref = ObjectRef::new(name.as_ref());
    match ns {
        Some(ns) => object_ref.within(ns.as_ref()),
        None => object_ref,
    }
}

pub(crate) fn to_delete_params(
    delete_options: DeleteOptions,
    dry_run: bool,
) -> Result<DeleteParams> {
    let preconditions = delete_options
        .preconditions
        .map(|preconditions| Preconditions {
            uid: preconditions.uid.clone(),
            resource_version: preconditions.resource_version.clone(),
        });

    let propagation_policy =
        if let Some(propagation_policy) = delete_options.propagation_policy.as_ref() {
            match propagation_policy.as_str() {
                "Orphan" => Some(PropagationPolicy::Orphan),
                "Background" => Some(PropagationPolicy::Background),
                "Foreground" => Some(PropagationPolicy::Foreground),
                other => return Err(eyre!("Unknown propagation policy: '{other}'")),
            }
        } else {
            None
        };

    Ok(DeleteParams {
        dry_run,
        grace_period_seconds: delete_options.grace_period_seconds.map(|x| x as _),
        preconditions,
        propagation_policy,
    })
}

#[macro_export]
macro_rules! instrumented {
    ($span:expr, $($tt:tt)+) => {{
        use ::tracing::Instrument;

        let span = $span;
        {
            $($tt)*
        }
        .instrument(span)
        .await
    }}
}

#[cfg(test)]
#[macro_export]
macro_rules! assert_matches {
    ($expr:expr, $($tt:tt)+) => {{
        let value = $expr;
        match value {
            $($tt)* => {}
            _ => ::std::panic!(
                "Expression = `{}`, value = `{:?}` does not match with pattern = `{}`.",
                stringify!($expr),
                value,
                stringify!($($tt)*),
            ),
        }
    }};
}

#[macro_export]
macro_rules! try_some {
    (@coalesce ($($h:tt)*)) => {
        $($h)*
    };
    (@coalesce ($($h:tt)*) ? $($t:tt)*) => {
        $crate::try_some!(@coalesce ($($h)*.as_ref()?) $($t)*)
    };
    (@coalesce ($($h:tt)*) $m:tt $($t:tt)*) => {
        $crate::try_some!(@coalesce ($($h)* $m) $($t)*)
    };

    (& $($tt:tt)*) => {
        (|| -> ::std::option::Option<_> {
            ::std::option::Option::Some(& $crate::try_some!(@coalesce () $($tt)*))
        })()
    };
    ($($tt:tt)*) => {
        (|| -> ::std::option::Option<_> {
            ::std::option::Option::Some($crate::try_some!(@coalesce () $($tt)*))
        })()
    };
}
