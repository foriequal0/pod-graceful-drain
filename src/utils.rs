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

pub(crate) fn to_delete_params(delete_options: &DeleteOptions) -> DeleteParams {
    let dry_run = delete_options.dry_run.iter().flatten().any(|x| x == "All");
    let grace_period_seconds = delete_options.grace_period_seconds.map(|x| x as _);
    let preconditions = delete_options
        .preconditions
        .as_ref()
        .map(|preconditions| Preconditions {
            uid: preconditions.uid.clone(),
            resource_version: preconditions.resource_version.clone(),
        });

    let propagation_policy = match delete_options.propagation_policy.as_deref() {
        Some("Orphan") => Some(PropagationPolicy::Orphan),
        Some("Background") => Some(PropagationPolicy::Background),
        Some("Foreground") => Some(PropagationPolicy::Foreground),
        Some(_) => {
            // TODO: report bug
            None
        }
        None => None,
    };

    DeleteParams {
        dry_run,
        grace_period_seconds,
        preconditions,
        propagation_policy,
    }
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
    (@coalesce ($($h:tt)*) *? $($t:tt)*) => {
        $crate::try_some!(@coalesce ($($h)*.as_deref()?) $($t)*)
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
    (@coalesce_mut ($($h:tt)*) *? $($t:tt)*) => {
        $crate::try_some!(@coalesce_mut ($($h)*.as_deref_mut()?) $($t)*)
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

#[cfg(test)]
#[macro_export]
macro_rules! from_json {
    ($($json:tt)+) => {{
        ::serde_json::from_value(::serde_json::json!($($json)+)).expect("Invalid json")
    }};
}

#[macro_export]
macro_rules! pub_if_test {
    ($($tt:tt)*) => {
        #[cfg(test)]
        pub $($tt)*;

        #[cfg(not(test))]
        $($tt)*;
    };
}

#[cfg(test)]
mod tests {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Preconditions;

    use super::*;

    #[test]
    pub fn smoke_test_to_delete_params() {
        let delete_options = DeleteOptions {
            dry_run: Some(vec!["All".to_owned()]),
            grace_period_seconds: Some(1234),
            preconditions: Some(Preconditions {
                uid: Some("uid".to_owned()),
                resource_version: Some("resource_version".to_owned()),
            }),
            propagation_policy: Some("Orphan".to_owned()),
            // other fields are ignored
            ..DeleteOptions::default()
        };

        let delete_params = to_delete_params(&delete_options);

        assert_eq!(
            delete_params,
            DeleteParams {
                dry_run: true,
                grace_period_seconds: Some(1234),
                preconditions: Some(kube::api::Preconditions {
                    uid: Some("uid".to_owned()),
                    resource_version: Some("resource_version".to_owned()),
                }),
                propagation_policy: Some(PropagationPolicy::Orphan),
            }
        )
    }
}
