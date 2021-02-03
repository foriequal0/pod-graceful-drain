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
It intercepts the deletion/eviction of a pod and deny them or delay the admission response to prevent the pod from getting terminated.
While doing so, it patches the pod to isolate it from the subsystems without terminating it.
By removing labels, endpoints remove the pod from their list, and load balancers deregister them.
The traffics are drained safely since the pod is still alive and can serve misdirected new traffics during the delay.
After that, the pod is removed asynchronously by it or continuing the delayed admission response.

I find that this is more 'graceful' than the brutal `sleep`. It can still feel like ad-hoc, and hacky, but the duct tapes are okay if they are hidden in the wall (until they leak).
