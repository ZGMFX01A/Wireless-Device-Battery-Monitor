"""鼠标级采样计划。时钟由调用者注入；不会创建线程、访问硬件或监听 USB。"""
from dataclasses import dataclass, field

CHARGING_SECONDS = 30.0
MAX_FAILURE_SECONDS = 300.0


@dataclass
class PollSlot:
    due: float
    failures: int = 0
    probes: list[float] = field(default_factory=list)


class MousePollPlan:
    def __init__(self):
        self.slots = {}

    def arm(self, key, now):
        """启动/恢复失败时至多补探测三次；错过的时间点不补执行。"""
        self.slots[key] = PollSlot(now, probes=[now + 1, now + 3, now + 5])

    def due(self, key, now):
        return key not in self.slots or self.slots[key].due <= now

    def record(self, key, now, *, success, charging=False, full=False, base=60):
        if key not in self.slots:
            self.arm(key, now)
        slot = self.slots[key]
        if success:
            slot.failures = 0
            slot.probes.clear()
            interval = min(base, CHARGING_SECONDS) if charging and not full else base
        else:
            slot.failures += 1
            interval = min(MAX_FAILURE_SECONDS, base * (2 ** min(slot.failures - 1, 3)))
        slot.probes[:] = [deadline for deadline in slot.probes if deadline > now]
        slot.due = slot.probes.pop(0) if slot.probes else now + interval

    def delay(self, now, auxiliary_due):
        deadline = min([auxiliary_due, *(slot.due for slot in self.slots.values())])
        return max(0.1, min(60.0, deadline - now))
