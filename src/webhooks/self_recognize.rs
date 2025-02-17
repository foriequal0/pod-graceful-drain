use k8s_openapi::api::authentication::v1::UserInfo;

use crate::downward_api::DownwardAPI;

pub fn is_my_serviceaccount(downward_api: &DownwardAPI, user_info: &UserInfo) -> bool {
    let Some(service_account_name) = &downward_api.pod_service_account_name else {
        return false;
    };
    let Some(namespace) = &downward_api.pod_namespace else {
        return false;
    };

    let Some(username) = &user_info.username else {
        return false;
    };
    let Some(groups) = &user_info.groups else {
        return false;
    };

    if !username.starts_with("system:serviceaccount:")
        || username != &format!("system:serviceaccount:{namespace}:{service_account_name}")
    {
        return false;
    }

    if !groups.iter().any(|group| group == "system:serviceaccounts") {
        return false;
    }

    if !groups.iter().any(|group| group == "system:authenticated") {
        return false;
    }

    let namespace_group = format!("system:serviceaccounts:{namespace}");
    if !groups.iter().any(|group| group == &namespace_group) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    use k8s_openapi::api::authentication::v1::UserInfo;

    #[test]
    pub fn test_is_my_serviceaccount_return_true() {
        let downward_api = DownwardAPI {
            pod_namespace: Some(String::from("some-namespace")),
            pod_service_account_name: Some(String::from("some-sa")),
            pod_name: None,
            pod_uid: None,
        };
        let user_info = UserInfo {
            username: Some(String::from("system:serviceaccount:some-namespace:some-sa")),
            groups: Some(vec![
                String::from("system:serviceaccounts"),
                String::from("system:serviceaccounts:some-namespace"),
                String::from("system:authenticated"),
            ]),
            extra: None,
            uid: None,
        };

        assert!(is_my_serviceaccount(&downward_api, &user_info));
    }

    #[test]
    pub fn test_is_my_serviceaccount_return_false() {
        let downward_api = DownwardAPI {
            pod_namespace: Some(String::from("some-namespace")),
            pod_service_account_name: Some(String::from("some-sa")),
            pod_name: None,
            pod_uid: None,
        };

        let user_info = UserInfo {
            username: Some(String::from(
                "system:serviceaccount:other-namespace:other-sa",
            )),
            groups: Some(vec![
                String::from("system:serviceaccounts"),
                String::from("system:serviceaccounts:other-namespace"),
                String::from("system:authenticated"),
            ]),
            extra: None,
            uid: None,
        };

        assert!(!is_my_serviceaccount(&downward_api, &user_info));
    }
}
