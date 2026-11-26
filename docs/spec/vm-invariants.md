# VM invariants

The internal rules no program observes whose breach would make a rule of the other parts false -- the write barrier, handles and their brand, what the collector traces, the rules of machine code -- each naming the rule it protects. Not written yet: the fifth batch of the specification writes it (`docs/design/specification.md`, "The order of building it").

## Detectors

```spec-table vm-detectors
| Detector | Kind | What it checks | Rules |
|---|---|---|---|
| MOVES | stress | every object moved at the run loop's safepoints | |
| CACHES | stress | every inline cache vacated at every safepoint | |
| COLLECT | stress | a collection at safepoints, the reservations given back | |
| SCAVENGE | stress | a scavenge at safepoints after a check of the remembered set | |
| WIDE | stress | every instruction in its wide form | |
| SUSPEND | stress | every activation suspended into its frame at safepoints | |
| JIT | stress | every body compiled at its first entry and entered at every push | |
| EXIT | stress | every poll in machine code taking its exit | |
| ZONE | stress | the code zone compacted at safepoints | |
| OPTIMIZE | stress | every body's optimization budget armed at 0 | |
| DEOPT | stress | optimized code leaving at interrupt points drawn afresh | |
| INVALIDATE | stress | optimized code's copies freed at every tenth interrupt point | |
| PAUSE | stress | a stop as for a debugger's pause at safepoints, the stack read | |
| BREAKPOINTS | stress | every statement start patched with a breakpoint that never stops | |
| STEP | stress | a burst of steps at safepoints, the stack read at every stop | |
```
