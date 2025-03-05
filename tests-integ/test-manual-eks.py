from pathlib import Path
from tempfile import TemporaryDirectory

import click

from utils.aws import EcrContext, get_test_aws_profile
from utils.eksctl import EksctlContext
from utils.docker import DockerContext, docker
from utils.helm import helm_install, helm
from utils.kubectl import KubectlContext, kubectl_stdin


@click.group()
def cli():
    pass


@cli.command()
def setup():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        eks_ctx.create_cluster()

        kubectl_ctx = KubectlContext(eks_ctx.get_kubeconfig(), "default")
        helm(
            kubectl_ctx,
            "upgrade",
            "--install",
            "aws-load-balancer-controller",
            "aws-load-balancer-controller",
            "--repo=https://aws.github.io/eks-charts",
            "--namespace=kube-system",
            f"--set=clusterName={eks_ctx.cluster_name}",
            "--set=serviceAccount.create=false",
            "--set=serviceAccount.name=aws-load-balancer-controller",
            "--wait=true",
        )

        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        ecr_ctx.create_repository()

        docker_ctx = DockerContext(
            tmp_path, ecr_ctx.login_password, ecr_ctx.proxy_endpoint
        )
        docker(
            docker_ctx,
            "image",
            "push",
            "--authfile",
            docker_ctx.authfile,
            "localhost/pod-graceful-drain:latest",
            f"{ecr_ctx.repository_uri}:latest",
        )
        helm_install(
            kubectl_ctx,
            "pod-graceful-drain",
            ecr_ctx.repository_uri,
        )

        kubectl_stdin(
            kubectl_ctx,
            "apply",
            "-f",
            "-",
            stdin="""
apiVersion: apps/v1
kind: Deployment
metadata:
  name: some-deployment
  labels:
    app: nginx
spec:
  replicas: 2
  selector:
    matchLabels:
      app: nginx
  template:
    metadata:
      labels:
        app: nginx
    spec:
      containers:
      - name: app
        image: public.ecr.aws/nginx/nginx
        ports:
        - name: http
          containerPort: 80
---
apiVersion: v1
kind: Service
metadata:
  name: some-service
spec:
  ports:
  - name: http
    port: 80
  selector:
    app: nginx
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: some-ingress
  annotations:
    alb.ingress.kubernetes.io/target-type: ip
    alb.ingress.kubernetes.io/scheme: internet-facing
    alb.ingress.kubernetes.io/listen-ports: '[{"HTTP": 80}]'
spec:
  ingressClassName: alb
  rules:
  - http:
      paths:
      - backend:
          service:
            name: some-service
            port:
              name: http
        pathType: Exact
        path: /
            """,
        )


@cli.command()
def write_kubeconfig():
    aws_profile = get_test_aws_profile()

    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        eks_ctx.write_kubeconfig()


@cli.command()
def update_image():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")

        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        docker_ctx = DockerContext(
            tmp_path, ecr_ctx.login_password, ecr_ctx.proxy_endpoint
        )
        docker(
            docker_ctx,
            "image",
            "push",
            "--authfile",
            docker_ctx.authfile,
            "localhost/pod-graceful-drain:latest",
            f"{ecr_ctx.repository_uri}:latest",
        )

        kubectl_ctx = KubectlContext(eks_ctx.get_kubeconfig(), "default")
        helm_install(
            kubectl_ctx,
            "pod-graceful-drain",
            ecr_ctx.repository_uri,
            values={"image.pullPolicy": "Always"},
        )


@cli.command()
def cleanup():
    aws_profile = get_test_aws_profile()
    with TemporaryDirectory(prefix="test-pgd-") as tempdir:
        tmp_path = Path(tempdir)
        eks_ctx = EksctlContext(tmp_path, aws_profile, "test-pgd")
        ecr_ctx = EcrContext(aws_profile, "test-pgd")
        ecr_ctx.cleanup_repository()
        eks_ctx.cleanup_cluster()


if __name__ == "__main__":
    cli()
