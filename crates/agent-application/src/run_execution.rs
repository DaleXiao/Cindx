/// One execution epoch either finishes the run or requests a fresh preparation
/// after an objective change such as user steering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunEpoch<Reprepare, Output> {
    Finished(Output),
    Reprepare(Reprepare),
}

/// Repreparation may produce another executable epoch or finish immediately at
/// a typed control boundary without dispatching another model request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentRunPreparation<Prepared, Output> {
    Prepared(Prepared),
    Finished(Output),
}

/// Side-effecting adapters implement the epoch and preparation operations. The
/// application layer owns the only production run/reprepare loop.
pub trait AgentRunExecutor {
    type Prepared;
    type Reprepare;
    type Output;
    type Error;

    fn execute_epoch(
        &mut self,
        prepared: Self::Prepared,
    ) -> Result<AgentRunEpoch<Self::Reprepare, Self::Output>, Self::Error>;

    fn reprepare(
        &mut self,
        handoff: Self::Reprepare,
    ) -> Result<AgentRunPreparation<Self::Prepared, Self::Output>, Self::Error>;
}

pub fn execute_agent_run<E>(
    executor: &mut E,
    mut prepared: E::Prepared,
) -> Result<E::Output, E::Error>
where
    E: AgentRunExecutor,
{
    loop {
        match executor.execute_epoch(prepared)? {
            AgentRunEpoch::Finished(output) => return Ok(output),
            AgentRunEpoch::Reprepare(handoff) => match executor.reprepare(handoff)? {
                AgentRunPreparation::Prepared(next) => prepared = next,
                AgentRunPreparation::Finished(output) => return Ok(output),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct ScriptedExecutor {
        epochs: VecDeque<AgentRunEpoch<u8, &'static str>>,
        preparations: VecDeque<AgentRunPreparation<u8, &'static str>>,
        executed: Vec<u8>,
        reprepared: Vec<u8>,
    }

    impl AgentRunExecutor for ScriptedExecutor {
        type Prepared = u8;
        type Reprepare = u8;
        type Output = &'static str;
        type Error = &'static str;

        fn execute_epoch(
            &mut self,
            prepared: Self::Prepared,
        ) -> Result<AgentRunEpoch<Self::Reprepare, Self::Output>, Self::Error> {
            self.executed.push(prepared);
            self.epochs.pop_front().ok_or("missing epoch")
        }

        fn reprepare(
            &mut self,
            handoff: Self::Reprepare,
        ) -> Result<AgentRunPreparation<Self::Prepared, Self::Output>, Self::Error> {
            self.reprepared.push(handoff);
            self.preparations.pop_front().ok_or("missing preparation")
        }
    }

    #[test]
    fn one_driver_owns_every_reprepare_epoch() {
        let mut executor = ScriptedExecutor {
            epochs: VecDeque::from([
                AgentRunEpoch::Reprepare(7),
                AgentRunEpoch::Finished("completed"),
            ]),
            preparations: VecDeque::from([AgentRunPreparation::Prepared(2)]),
            executed: Vec::new(),
            reprepared: Vec::new(),
        };

        assert_eq!(execute_agent_run(&mut executor, 1), Ok("completed"));
        assert_eq!(executor.executed, vec![1, 2]);
        assert_eq!(executor.reprepared, vec![7]);
    }

    #[test]
    fn control_stop_during_reprepare_finishes_without_another_epoch() {
        let mut executor = ScriptedExecutor {
            epochs: VecDeque::from([AgentRunEpoch::Reprepare(9)]),
            preparations: VecDeque::from([AgentRunPreparation::Finished("paused")]),
            executed: Vec::new(),
            reprepared: Vec::new(),
        };

        assert_eq!(execute_agent_run(&mut executor, 1), Ok("paused"));
        assert_eq!(executor.executed, vec![1]);
        assert_eq!(executor.reprepared, vec![9]);
    }
}
