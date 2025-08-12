#include <linux/types.h>
#include <linux/oom.h>
#include <linux/sched.h>
#include <linux/gfp.h>
#include <linux/mm.h>
#include <stddef.h>
#include <stdint.h>

// Minimal definitions for oom_control and task_struct
// These are simplified to only include fields necessary for the eBPF program

struct task_struct {
    pid_t pid;
    pid_t tgid;
    char comm[16];
    // Other fields are omitted for brevity
};

struct oom_control {
    /* Used to determine cpuset */
    struct zonelist *zonelist;

    /* Used to determine mempolicy */
    nodemask_t *nodemask;

    /* Memory cgroup in which oom is invoked, or NULL for global oom */
    struct mem_cgroup *memcg;

    /* Used to determine cpuset and node locality requirement */
    const gfp_t gfp_mask;

    /*
     * order == -1 means the oom kill is required by sysrq, otherwise only
     * for display purposes.
     */
    const int order;

    /* Used by oom implementation, do not set */
    unsigned long totalpages;
    struct task_struct *chosen;
    long chosen_points;

    /* Used to print the constraint info. */
    enum oom_constraint constraint;

    // Fields relevant to OOM event
    unsigned long total_vm;
    unsigned long rss;
    u64 memcg_id;
    struct task_struct *victim;
};
