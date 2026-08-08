use crate::api::course::{build_tasks, fetch_course_units, fetch_unit, select_tasks, GroupTask};
use anyhow::Result;

#[derive(Debug, Default)]
pub struct Plan {
    pub units: Vec<UnitPlan>,
    pub total: usize,
    pub todo: usize,
}

#[derive(Debug)]
pub struct UnitPlan {
    pub unit_id: String,
    pub tasks: Vec<GroupTask>,
    pub todo: usize,
}

pub async fn plan_course(session: &crate::api::session::Session) -> Result<Plan> {
    let compulsory_only = session.cfg().compulsory_only();
    let units = fetch_course_units(session).await?;
    let mut plan = Plan::default();
    for uid in &units {
        let rt = fetch_unit(session, uid).await?;
        let tasks = select_tasks(&build_tasks(uid, &rt), compulsory_only);
        let todo = tasks.iter().filter(|t| !t.passed).count();
        plan.units.push(UnitPlan {
            unit_id: uid.clone(),
            tasks,
            todo,
        });
        plan.total += todo;
        plan.todo += todo;
    }
    Ok(plan)
}

impl Plan {
    pub fn pending_tasks(&self) -> Vec<&GroupTask> {
        self.units
            .iter()
            .flat_map(|u| u.tasks.iter().filter(|t| !t.passed))
            .collect()
    }
}