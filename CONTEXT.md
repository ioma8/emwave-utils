# Domain context

## Terms

- **Beat sample**: One emWave2 observation with an IBI, heart rate, artifact flag, physiological time, and receipt time.
- **NN series**: The ordered artifact-free inter-beat intervals used for HRV analysis.
- **Session**: One continuous emWave2 capture from reader start until disconnect or application shutdown.
- **Trial**: One paced-breathing assessment epoch at a prescribed rate.
- **Rest period**: The natural-breathing interval between automatic assessment trials.
- **Cardiac candidate**: A trial rate whose PPG-derived target-frequency response and LF peak satisfy the app's heuristic criteria; not a validated personal resonance frequency without synchronized respiration.
- **Session archive**: The persisted collection of completed session summaries and their beat samples.
