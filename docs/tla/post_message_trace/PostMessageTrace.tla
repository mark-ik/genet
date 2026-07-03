---- MODULE PostMessageTrace ----
EXTENDS PostMessageTraceData

VARIABLE pc

Witness == INSTANCE PostMessage WITH Trace <- Trace, pc <- pc

Spec == Witness!Spec
TypeOK == Witness!TypeOK
UniqueEnqueueIds == Witness!UniqueEnqueueIds
ExactlyOnceDelivery == Witness!ExactlyOnceDelivery
AsyncDelivery == Witness!AsyncDelivery
DeliveriesRunInsideEventLoop == Witness!DeliveriesRunInsideEventLoop
TimerBackedDelivery == Witness!TimerBackedDelivery

====
