# RemoteX v1.3 connection performance

RemoteX treats perceived connection time as separate milestones rather than one
ambiguous duration. Relay readiness is the first usable transport milestone;
Direct upgrade is allowed to complete later without blocking the session UI.

## Milestones and provenance

| Milestone | Clock | Status in v1.3 |
| --- | --- | --- |
| Session request | Controller monotonic clock | locally measurable |
| Authorization complete | Controller monotonic clock | locally measurable |
| Relay ready | Controller/Agent monotonic clock | locally measurable |
| First usable frame | Controller monotonic clock | locally measurable |
| Direct attempt/established | local monotonic clock | locally measurable |
| Agent claim | Control Server wall clock | server-side audit only |
| Input ready | Controller monotonic clock | equivalent to Relay ready |

Do not subtract timestamps from different computers. RemoteX therefore does not
claim a measured end-to-end frame age until clock offset and uncertainty are
calibrated. Diagnostics display `—` for that value. Capture, encode, decode, and
WebView render durations are measured inside their own stages. QUIC supplies RTT
and cumulative lost/sent packet counts. Decode-budget queue pressure is labeled
estimated because it is not a direct queue-depth counter.

## Expected behavior

1. Authorization completes before any data-plane credential is useful.
2. Relay pairs and the UI becomes connected immediately.
3. Direct checks use a bounded background window.
4. An authenticated Direct stream switches only after the ordered four-message
   barrier. Failure leaves the usable E2EE Relay path intact.
5. Transient failure uses at most five jittered recovery attempts (nominally
   0.5, 1, 2, 4, and 8 seconds), bounded again by recovery expiry.

Continuous setup metrics are not uploaded or persisted. Operational Relay
counters are local aggregate values without peer-address labels.

