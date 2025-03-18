import random
import string
import subprocess
import datetime
from pathlib import Path

import boto3

from .subprocess_util import handle_error, print_command, to_bytes


class EksctlContext:
    def __init__(self, tmp_path: Path, aws_profile: str, cluster_name: str):
        self.aws_profile = aws_profile
        self.cluster_name = cluster_name if cluster_name else _random_name()
        self._kubeconfig = str(tmp_path / "kubeconfig")

    def create_cluster(self):
        region_name = boto3.Session(profile_name=self.aws_profile).region_name
        eksctl_stdin(
            self,
            "create",
            "cluster",
            # not to messing with global kubeconfig.
            "--kubeconfig",
            self._kubeconfig,
            "-f",
            "-",
            stdin=f"""
apiVersion: eksctl.io/v1alpha5
kind: ClusterConfig
metadata:
  name: {self.cluster_name}
  version: "1.29"
  region: "{region_name}"
  tags:
    note: "test cluster by pod-graceful-drain test"
    created-at: "{datetime.datetime.now(datetime.UTC)}"
managedNodeGroups:
  - name: managed-ng-1
    minSize: 2
    iam: 
      withAddonPolicies:
        awsLoadBalancerController: true
cloudWatch:
  clusterLogging:
    enableTypes: ["all"]
iam:
  withOIDC: true
  serviceAccounts:
  - metadata:
      name: aws-load-balancer-controller
      namespace: kube-system
    wellKnownPolicies:
      awsLoadBalancerController: true
""",
        )

    def cleanup_cluster(self):
        eksctl(self, "delete", "cluster", "--name", self.cluster_name, "--wait")

    def get_kubeconfig(self):
        eksctl(
            self,
            "utils",
            "write-kubeconfig",
            "--cluster",
            self.cluster_name,
            "--kubeconfig",
            self._kubeconfig,
        )

        return self._kubeconfig

    def write_kubeconfig(self):
        eksctl(
            self,
            "utils",
            "write-kubeconfig",
            "--cluster",
            self.cluster_name,
        )

    def __enter__(self):
        self.create_cluster()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.cleanup_cluster()


def _get_command(ctx: EksctlContext, /, *args):
    command = ["eksctl"]
    command.extend(["--profile", ctx.aws_profile])
    command.extend(args)
    return command


def eksctl(ctx: EksctlContext, /, *args):
    command = _get_command(ctx, *args)
    print_command(command)
    result = subprocess.run(command)
    handle_error(result)


def eksctl_stdin(ctx: EksctlContext, /, *args, stdin):
    command = _get_command(ctx, *args)
    print_command([*command, f"<<<EOF\n{stdin}\nEOF"])
    result = subprocess.run(
        command,
        input=to_bytes(stdin),
    )
    handle_error(result)


def _random_name():
    alnum = string.ascii_lowercase + string.digits
    suffix = "".join(random.choice(alnum) for _ in range(8))
    return "test-pgd-" + suffix
