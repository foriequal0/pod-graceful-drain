pub const X: i32 = 1234;

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

    (& mut $($tt:tt)*) => {
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
