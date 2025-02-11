![GitHub tag (latest SemVer)](https://img.shields.io/github/v/tag/foriequal0/pod-graceful-drain) ![Helm chart version](https://img.shields.io/badge/dynamic/yaml?label=Helm%20chart&query=%24.entries[%22pod-graceful-drain%22][0].version&url=https%3A%2F%2Fforiequal0.github.io%2Fpod-graceful-drain%2Findex.yaml)

# Pod Graceful Drain

You don't need `lifecycle: { preStop: { exec: { command: ["sleep", "30"] } } }`

## Installation

```shell
helm install \
  --repo https://foriequal0.github.io/pod-graceful-drain\
  --namespace kube-system \
  pod-graceful-drain \
  pod-graceful-drain
```

## What is this?

Have you ever suffered from getting 5xx errors on your load balancer when you roll out new deployment?
Have you ever applied this ugly mitigation even if your app is able to shut down gracefully?

```yaml
lifecycle:
  preStop:
    exec:
      command: ["sleep", "30"]
```

As far as I know, in Kubernetes, there is no facility to notify pod dependent subsystems that the pod is about to be terminated and to reach a consensus that the pod is okay to be terminated.
So during the deployment rollout, a pod is terminated first, and the subsystems reconcile after that.
There is an inevitable delay between the pod termination and the reconciliation.
Eventually, endpoints remove the pod from their lists, then load balancer controllers deregister and start to drain the traffic.
But, what's the point of draining when the pod is already terminated?
It is too late when the deregistration is fully propagated. This is the cause of load balancer 5xx errors. You might reduce the delay, but can't eliminate it.

So that's why everyone suggests `sleep 30` while saying it is an ugly hack regardless of being able to terminate gracefully.
It delays `SIGTERM` to the app while setting the pod to the terminating state, so the dependent subsystems could do reconciliation during the delay.
However, sometimes, "sleep" command might not be available on some containers.
It might be needed to apply mistake-prone patches to some chart distributions.
And it is ugly. It doesn't seem to be solved in a foreseeable future, and related issues are getting closed due to the inactivity by the bots.

`pod-graceful-drain` solves this by abusing [admission webhooks][admission-webhook].
It intercepts the deletion/eviction of a pod deletion/eviction process to prevent the pod from getting terminated for a period.
It'll take appropriate methods to delay the pod deletion: deny the admission, response the admission very slowly, mutate the eviction request to dry-run, etc.
Then the pod will be eventually terminated by the controller after designated timeouts.
With this delay, traffics are drained safely since the pod is still alive and can serve misdirected new traffics.

[admission-webhook]: https://kubernetes.io/docs/reference/access-authn-authz/extensible-admission-controllers

Another goal of it is making sure it won't affect common tasks such as deployment rollout, or `kubectl drain`.
By removing labels, which isolates the pod from the replicasets, rollout process will continue as the pod was terminated, without actually terminating it.
It modifies the requested `pods/eviction`, which usually made during the `kubectl drain`, to be dry-run, then it isolates and eventually terminates the pod.

I find that this is more 'graceful' than the brutal `sleep`. It can still feel like ad-hoc, and hacky, but the duct tapes are okay if they are hidden in the wall (until they leak).

## Development

### Testing prerequisite

Integration test uses [`kubectl`](https://kubernetes.io/docs/reference/kubectl/), [`kind`](https://kind.sigs.k8s.io),
[`helm`](https://helm.sh) binary at `PATH`, and optionally [`skaffold`](https://skaffold.dev).

```shell
cargo test
rye test
```

### Windows

If you're using Windows, you might find it is much easier to compile dependencies with `msvc` toolchain.

```shell
rustup override set stable-x86_64-pc-windows-msvc
```
