mod testutils;

use std::collections::BTreeMap;
use std::time::Duration;

use k8s_openapi::api::core::v1::Service;
use k8s_openapi::api::networking::v1::Ingress;
use kube::runtime::reflector::ObjectRef;
use kube::ResourceExt;

use crate::testutils::context::{within_test_namespace, TestContext};

use pod_graceful_drain::{start_reflectors, try_some, Config, Stores};

fn start_test_reflector(context: &TestContext) -> Stores {
    let config = Config {
        delete_after: Duration::from_secs(10),
        experimental_general_ingress: true,
    };

    start_reflectors(
        &context.api_resolver,
        &config,
        &Default::default(),
        &context.shutdown,
    )
    .unwrap()
}

#[tokio::test]
async fn should_reflect_pod() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        kubectl!(
            &context,
            [
                "run",
                "some-pod",
                "--image=public.ecr.aws/docker/library/busybox",
                "--labels=some-label=some-value",
                "--",
                "sleep",
                "9999"
            ]
        );

        let pod = eventually_some!(
            stores.get_pod(&ObjectRef::new("some-pod").within(&context.namespace))
        );

        assert_eq!(
            pod.metadata.labels.as_ref(),
            Some(&{
                let mut selector = BTreeMap::new();
                selector.insert(String::from("some-label"), String::from("some-value"));
                selector
            }),
            "should reflect labels"
        );
    })
    .await;
}

#[tokio::test]
async fn should_reflect_pod_delete() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        kubectl!(
            &context,
            [
                "run",
                "some-pod",
                "--image=public.ecr.aws/docker/library/busybox",
                "--",
                "sleep",
                "9999"
            ]
        );
        let pod = eventually_some!(
            stores.get_pod(&ObjectRef::new("some-pod").within(&context.namespace))
        );

        kubectl!(&context, ["delete", "pod", "some-pod"]);

        assert!(eventually!(stores
            .get_pod(&ObjectRef::from_obj(&pod))
            .is_none()));
    })
    .await;
}

#[tokio::test]
async fn should_reflect_pod_label_edit() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        kubectl!(
            &context,
            [
                "run",
                "some-pod",
                "--image=public.ecr.aws/docker/library/busybox",
                "--",
                "sleep",
                "9999"
            ]
        );

        let pod = eventually_some!(
            stores.get_pod(&ObjectRef::new("some-pod").within(&context.namespace))
        );

        kubectl!(
            &context,
            ["label", "pod", "some-pod", "some-label=edited-value"]
        );

        assert_eq!(
            eventually_some!(try_some!((stores
                .get_pod(&ObjectRef::from_obj(&pod))?
                .labels()
                .get("some-label")?)
            .clone())),
            String::from("edited-value")
        );
    })
    .await;
}

#[tokio::test]
async fn should_reflect_service() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Service,
            "\
metadata:
  name: some-service
spec:
  ports:
  - port: 80
    protocol: TCP
  selector:
    some-label: some-value"
        );

        let service = eventually_some!(
            stores.get_service(&ObjectRef::new("some-service").within(&context.namespace))
        );

        assert_eq!(
            try_some!(service.spec?.selector?),
            Some(&{
                let mut selector = BTreeMap::new();
                selector.insert(String::from("some-label"), String::from("some-value"));
                selector
            }),
            "should reflect selectors"
        );
    })
    .await;
}

#[tokio::test]
async fn should_reflect_service_spec_edit() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Service,
            "\
metadata:
  name: some-service
spec:
  ports:
  - port: 80
    protocol: TCP
  selector:
    some-label: some-value"
        );

        let service = eventually_some!(
            stores.get_service(&ObjectRef::new("some-service").within(&context.namespace))
        );

        apply_yaml!(
            &context,
            Service,
            "\
metadata:
  name: some-service
spec:
  ports:
  - port: 80
    protocol: TCP
  selector:
    some-label: edited-value"
        );

        assert_eq!(
            eventually_some!({
                let label = try_some!(stores
                    .get_service(&ObjectRef::from_obj(&service))?
                    .spec?
                    .selector?
                    .get("some-label")
                    .cloned())
                .flatten();
                match label {
                    Some(str) if str == "some-value" => None,
                    _ => label,
                }
            }),
            String::from("edited-value")
        );
    })
    .await;
}

#[tokio::test]
async fn should_reflect_service_delete() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Service,
            "\
metadata:
  name: some-service
spec:
  ports:
  - port: 80
    protocol: TCP
  selector:
    some-label: some-value"
        );

        let service = eventually_some!(
            stores.get_service(&ObjectRef::new("some-service").within(&context.namespace))
        );
        kubectl!(&context, ["delete", "service", "some-service"]);

        assert!(eventually!(stores
            .get_service(&ObjectRef::from_obj(&service))
            .is_none()));
    })
    .await;
}

#[tokio::test]
async fn should_reflect_ingress() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Ingress,
            "\
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: test
            port:
              number: 80"
        );

        let ingress = eventually_some!({
            stores
                .ingresses()
                .into_iter()
                .find(|ingress| ingress.name_any() == "some-ingress")
        });

        assert_eq!(
            try_some!(ingress.spec?.rules?[0].http?.paths[0]
                .backend
                .service?
                .name
                .clone()),
            Some(String::from("test"))
        );
    })
    .await;
}

#[tokio::test]
async fn should_reflect_ingress_delete() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Ingress,
            "\
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: test
            port:
              number: 80"
        );

        eventually_some!({
            stores
                .ingresses()
                .into_iter()
                .find(|ingress| ingress.name_any() == "some-ingress")
        });

        kubectl!(&context, ["delete", "ingress", "some-ingress"]);

        assert!(eventually!(stores.ingresses().is_empty()));
    })
    .await;
}

#[tokio::test]
async fn should_reflect_ingress_spec_edit() {
    within_test_namespace(|context| async move {
        let stores = start_test_reflector(&context);

        apply_yaml!(
            &context,
            Ingress,
            "\
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: test
            port:
              number: 80"
        );

        let ingress = eventually_some!(stores
            .ingresses()
            .into_iter()
            .find(|ingress| ingress.name_any() == "some-ingress"));

        apply_yaml!(
            &context,
            Ingress,
            "\
metadata:
  name: some-ingress
spec:
  rules:
  - http:
      paths:
      - path: /
        pathType: Prefix
        backend:
          service:
            name: another-service
            port:
              number: 80"
        );

        let old_label = try_some!(ingress.spec?.rules?[0].http?.paths[0]
            .backend
            .service?
            .name
            .clone());
        assert_eq!(
            eventually_some!({
                let new_ingress = try_some!(stores
                    .ingresses()
                    .into_iter()
                    .find(|ing| ing.name_any() == ingress.name_any()))
                .unwrap();
                let new_label = try_some!(new_ingress?.spec?.rules?[0].http?.paths[0]
                    .backend
                    .service?
                    .name
                    .clone());
                if new_label == old_label {
                    None
                } else {
                    new_label
                }
            }),
            String::from("another-service")
        );
    })
    .await;
}
