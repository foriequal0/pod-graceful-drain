use eyre::Result;
use eyre::eyre;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::DeleteOptions;
use kube::Resource;
use kube::api::{DeleteParams, Preconditions, PropagationPolicy};
use kube::runtime::reflector::ObjectRef;

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
    }}
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

    (@coalesce_mut ($($h:tt)*)) => {
        $($h)*
    };
    (@coalesce_mut ($($h:tt)*) ? $($t:tt)*) => {
        $crate::try_some!(@coalesce_mut ($($h)*.as_mut()?) $($t)*)
    };
    (@coalesce_mut ($($h:tt)*) $m:tt $($t:tt)*) => {
        $crate::try_some!(@coalesce_mut ($($h)* $m) $($t)*)
    };

    (mut $($tt:tt)*) => {
        {
            fn call<R>(f: impl FnOnce() ->::std::option::Option<R>) -> ::std::option::Option <R>{
                f()
            }
            call(|| {
                ::std::option::Option::Some($crate::try_some!(@coalesce_mut () $($tt)*))
            })
        }
    };
    (&mut $($tt:tt)*) => {
        {
            fn call<R>(f: impl FnOnce() ->::std::option::Option<R>) -> ::std::option::Option<R>
            {
                f()
            }
            call(|| {
                ::std::option::Option::Some(&mut $crate::try_some!(@coalesce_mut () $($tt)*))
            })
        }
    };
    (& $($tt:tt)*) => {
        {
            fn call<R>(f: impl FnOnce() ->::std::option::Option<R>) -> ::std::option::Option <R>{
                f()
            }
            call(|| {
                ::std::option::Option::Some(&$crate::try_some!(@coalesce () $($tt)*))
            })
        }
    };
    ($($tt:tt)*) => {
        {
            fn call<R>(f: impl FnOnce() ->::std::option::Option<R>) -> ::std::option::Option <R>{
                f()
            }
            call(|| {
                ::std::option::Option::Some($crate::try_some!(@coalesce () $($tt)*))
            })
        }
    };
}
