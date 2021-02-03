package elbv2

import (
	"context"
	awssdk "github.com/aws/aws-sdk-go/aws"
	elbv2sdk "github.com/aws/aws-sdk-go/service/elbv2"
	"github.com/go-logr/logr"
	"k8s.io/apimachinery/pkg/util/sets"
	"sigs.k8s.io/aws-load-balancer-controller/pkg/aws/services"
	"sigs.k8s.io/aws-load-balancer-controller/pkg/model/core"
	elbv2model "sigs.k8s.io/aws-load-balancer-controller/pkg/model/elbv2"
	"strconv"
)

// NewListenerRuleSynthesizer constructs new listenerRuleSynthesizer.
func NewListenerRuleSynthesizer(elbv2Client services.ELBV2, lrManager ListenerRuleManager, logger logr.Logger, stack core.Stack) *listenerRuleSynthesizer {
	return &listenerRuleSynthesizer{
		elbv2Client: elbv2Client,
		lrManager:   lrManager,
		logger:      logger,
		stack:       stack,
	}
}

type listenerRuleSynthesizer struct {
	elbv2Client services.ELBV2
	lrManager   ListenerRuleManager
	logger      logr.Logger

	stack core.Stack
}

func (s *listenerRuleSynthesizer) Synthesize(ctx context.Context) error {
	var resLRs []*elbv2model.ListenerRule
	s.stack.ListResources(&resLRs)
	resLRsByLSARN, err := mapResListenerRuleByListenerARN(resLRs)
	if err != nil {
		return err
	}

	var resLSs []*elbv2model.Listener
	s.stack.ListResources(&resLSs)
	for _, resLS := range resLSs {
		lsARN, err := resLS.ListenerARN().Resolve(ctx)
		if err != nil {
			return err
		}
		resLRs := resLRsByLSARN[lsARN]
		if err := s.synthesizeListenerRulesOnListener(ctx, lsARN, resLRs); err != nil {
			return err
		}
	}
	return nil
}

func (s *listenerRuleSynthesizer) PostSynthesize(ctx context.Context) error {
	// nothing to do here.
	return nil
}

func (s *listenerRuleSynthesizer) synthesizeListenerRulesOnListener(ctx context.Context, lsARN string, resLRs []*elbv2model.ListenerRule) error {
	sdkLRs, err := s.findSDKListenersRulesOnLS(ctx, lsARN)
	if err != nil {
		return err
	}

	matchedResAndSDKLRs, unmatchedResLRs, unmatchedSDKLRs := matchResAndSDKListenerRules(resLRs, sdkLRs)
	for _, sdkLR := range unmatchedSDKLRs {
		if err := s.lrManager.Delete(ctx, sdkLR); err != nil {
			return err
		}
	}
	for _, resLR := range unmatchedResLRs {
		lrStatus, err := s.lrManager.Create(ctx, resLR)
		if err != nil {
			return err
		}
		resLR.SetStatus(lrStatus)
	}
	for _, resAndSDKLR := range matchedResAndSDKLRs {
		lsStatus, err := s.lrManager.Update(ctx, resAndSDKLR.resLR, resAndSDKLR.sdkLR)
		if err != nil {
			return err
		}
		resAndSDKLR.resLR.SetStatus(lsStatus)
	}
	return nil
}

// findSDKListenersRulesOnLS returns the listenerRules configured on Listener.
func (s *listenerRuleSynthesizer) findSDKListenersRulesOnLS(ctx context.Context, lsARN string) ([]*elbv2sdk.Rule, error) {
	req := &elbv2sdk.DescribeRulesInput{
		ListenerArn: awssdk.String(lsARN),
	}
	rules, err := s.elbv2Client.DescribeRulesAsList(ctx, req)
	if err != nil {
		return nil, err
	}
	nonDefaultRules := make([]*elbv2sdk.Rule, 0, len(rules))
	for _, rule := range rules {
		if awssdk.BoolValue(rule.IsDefault) {
			continue
		}
		nonDefaultRules = append(nonDefaultRules, rule)
	}
	return nonDefaultRules, nil
}

type resAndSDKListenerRulePair struct {
	resLR *elbv2model.ListenerRule
	sdkLR *elbv2sdk.Rule
}

func matchResAndSDKListenerRules(resLRs []*elbv2model.ListenerRule, sdkLRs []*elbv2sdk.Rule) ([]resAndSDKListenerRulePair, []*elbv2model.ListenerRule, []*elbv2sdk.Rule) {
	var matchedResAndSDKLRs []resAndSDKListenerRulePair
	var unmatchedResLRs []*elbv2model.ListenerRule
	var unmatchedSDKLRs []*elbv2sdk.Rule

	resLRByPriority := mapResListenerRuleByPriority(resLRs)
	sdkLRByPriority := mapSDKListenerRuleByPriority(sdkLRs)
	resLRPriorities := sets.Int64KeySet(resLRByPriority)
	sdkLRPriorities := sets.Int64KeySet(sdkLRByPriority)
	for _, priority := range resLRPriorities.Intersection(sdkLRPriorities).List() {
		resLR := resLRByPriority[priority]
		sdkLR := sdkLRByPriority[priority]
		matchedResAndSDKLRs = append(matchedResAndSDKLRs, resAndSDKListenerRulePair{
			resLR: resLR,
			sdkLR: sdkLR,
		})
	}
	for _, priority := range resLRPriorities.Difference(sdkLRPriorities).List() {
		unmatchedResLRs = append(unmatchedResLRs, resLRByPriority[priority])
	}
	for _, priority := range sdkLRPriorities.Difference(resLRPriorities).List() {
		unmatchedSDKLRs = append(unmatchedSDKLRs, sdkLRByPriority[priority])
	}

	return matchedResAndSDKLRs, unmatchedResLRs, unmatchedSDKLRs
}

func mapResListenerRuleByPriority(resLRs []*elbv2model.ListenerRule) map[int64]*elbv2model.ListenerRule {
	resLRByPriority := make(map[int64]*elbv2model.ListenerRule, len(resLRs))
	for _, resLR := range resLRs {
		resLRByPriority[resLR.Spec.Priority] = resLR
	}
	return resLRByPriority
}

func mapSDKListenerRuleByPriority(sdkLRs []*elbv2sdk.Rule) map[int64]*elbv2sdk.Rule {
	sdkLRByPriority := make(map[int64]*elbv2sdk.Rule, len(sdkLRs))
	for _, sdkLR := range sdkLRs {
		priority, _ := strconv.ParseInt(awssdk.StringValue(sdkLR.Priority), 10, 64)
		sdkLRByPriority[priority] = sdkLR
	}
	return sdkLRByPriority
}

func mapResListenerRuleByListenerARN(resLRs []*elbv2model.ListenerRule) (map[string][]*elbv2model.ListenerRule, error) {
	resLRsByLSARN := make(map[string][]*elbv2model.ListenerRule, len(resLRs))
	ctx := context.Background()
	for _, lr := range resLRs {
		lsARN, err := lr.Spec.ListenerARN.Resolve(ctx)
		if err != nil {
			return nil, err
		}
		resLRsByLSARN[lsARN] = append(resLRsByLSARN[lsARN], lr)
	}
	return resLRsByLSARN, nil
}
