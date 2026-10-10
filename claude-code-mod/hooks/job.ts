export const LOG = '~/Library/Logs/ai-usagebar-desktop-switch.log'
export const JOB_PREFIX = 'com.akitaonrails.ai-usagebar.desktop-switch'
// Runs as a launchd job, outside Claude's process tree. `launchctl submit` restarts a job that
// fails, so the job always removes itself when the switch ends, success or not.
export const JOB_SCRIPT =
  'job=$1; shift; echo "== $(/bin/date) $*"; "$@"; echo "== exit $?"; /bin/launchctl remove "$job"'
