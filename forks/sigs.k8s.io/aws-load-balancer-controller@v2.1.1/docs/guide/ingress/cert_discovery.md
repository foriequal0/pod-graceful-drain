# Certificate Discovery
TLS certificates for ALB Listeners can be automatically discovered with hostnames from Ingress resources if the [`alb.ingress.kubernetes.io/certificate-arn`](annotations.md#certificate-arn) annotation is not specified.

The controller will attempt to discover TLS certificates from the `tls` field in Ingress and `host` field in Ingress rules.

!!!note ""
    You need to explicitly specify to use HTTPS listener with [listen-ports](annotations.md#listen-ports) annotation.

## Discover via Ingress tls

!!!example
        - attaches certs for `www.example.com` to the ALB
            ```yaml
            apiVersion: extensions/v1beta1
            kind: Ingress
            metadata:
            namespace: default
            name: ingress
            annotations:
              kubernetes.io/ingress.class: alb
              alb.ingress.kubernetes.io/listen-ports: '[{"HTTPS":443}]'
            spec:
              tls:
              - hosts:
                - www.example.com
              rules:
              - http:
                  paths:
                  - path: /users/*
                    backend:
                      serviceName: user-service
                      servicePort: 80
            ```


## Discover via Ingress rule host.

!!!example
        - attaches a cert for `dev.example.com` or `*.example.com` to the ALB
            ```yaml
            apiVersion: extensions/v1beta1
            kind: Ingress
            metadata:
            namespace: default
            name: ingress
            annotations:
              kubernetes.io/ingress.class: alb
              alb.ingress.kubernetes.io/listen-ports: '[{"HTTPS":443}]'
            spec:
            rules:
            - host: dev.example.com
              http:
                paths:
                - path: /users/*
                backend:
                  serviceName: user-service
                  servicePort: 80
            ```
