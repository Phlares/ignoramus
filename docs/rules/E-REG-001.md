# E-REG-001

Severity: error

## Problem

`register` needs a person at a terminal, and standard input is not a terminal.

## Fix

Ask the person to run `ignorarium register <spec>` in an interactive terminal and type the approval themselves. Do not pipe or script the approval.

## Why

Registration locks the plan. The person who owns the study must approve it by reading the hash, not a program acting for them.

## Applies to

- the command line, not a spec field
