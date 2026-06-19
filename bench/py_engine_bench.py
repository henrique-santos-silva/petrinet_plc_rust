"""Engine throughput benchmark (Python side).

Mirrors backend/examples/engine_bench.rs: same P0 <-> P1 cycle, same hot path
(get_transition_chosen_to_fire + fire), no web layer. Uses the ORIGINAL engine
classes from the petrinet_plc repo.

Run:  PYTHONPATH=/home/pessoal/petrinet_plc python3 bench/py_engine_bench.py [iterations]
"""
import sys
import time

from src.implementation.petri_net_subcomponents import (
    Place,
    InstantaneousTransition,
    Arc,
    TransitionsCollection,
)


class FakeIO:
    """Minimal IO handler: inputs never change (matches io_updated=False)."""

    @property
    def has_been_updated(self):
        return False


def main():
    iterations = int(sys.argv[1]) if len(sys.argv) > 1 else 1_000_000

    p0 = Place("P0", capacity=1, marking=1)
    p1 = Place("P1", capacity=1, marking=0)
    t0 = InstantaneousTransition("T0", rate=1, priority=1)
    t1 = InstantaneousTransition("T1", rate=1, priority=1)
    Arc("a0", source_node=p0, target_node=t0, weight=1, is_inhibitor=False)
    Arc("a1", source_node=t0, target_node=p1, weight=1, is_inhibitor=False)
    Arc("a2", source_node=p1, target_node=t1, weight=1, is_inhibitor=False)
    Arc("a3", source_node=t1, target_node=p0, weight=1, is_inhibitor=False)

    coll = TransitionsCollection(transitions=[t0, t1], io_handler=FakeIO())

    firings = 0
    start = time.perf_counter()
    for _ in range(iterations):
        t = coll.get_transition_chosen_to_fire()
        if t is not None:
            t.fire()
            firings += 1
    elapsed = time.perf_counter() - start

    print("PYTHON engine benchmark")
    print(f"  iterations : {iterations}")
    print(f"  firings    : {firings}")
    print(f"  elapsed    : {elapsed:.4f} s")
    print(f"  firings/s  : {firings / elapsed:.0f}")


if __name__ == "__main__":
    main()
