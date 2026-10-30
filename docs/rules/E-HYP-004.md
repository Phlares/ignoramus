# E-HYP-004

Severity: error

## Problem

A confirmatory hypothesis has no alpha share above 0 and at most 1, or the shares do not sum to 1.

## Fix

Give each hypothesis with a claim an alpha_share above 0 and at most 1, so that the shares of the study sum to 1. A study with one hypothesis sets alpha_share = 1.0.

## Why

The study's level is split between its claims. Shares that sum to more than 1 would let the study reject more often than its level allows, and shares that sum to less waste power.

## Applies to

- `question.hypotheses[*].alpha_share` (named:alpha-shares), when mode = confirmatory
- `question.hypotheses` (named:alpha-shares), when mode = confirmatory
