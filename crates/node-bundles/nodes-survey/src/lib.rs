//! Survey analysis DAG node bundle (R survey package port).

pub mod survey_calibrate;
pub mod survey_common;
pub mod survey_describe;
pub mod survey_model;
pub mod survey_rcs;
pub mod survey_survival;
pub mod survey_test;
pub mod survey_utility;

use dag_core::{NodePlugin, NodeRegistry};

pub struct Plugin;
impl NodePlugin for Plugin {
    fn name(&self) -> &'static str {
        "survey"
    }
    fn register(&self, registry: &mut NodeRegistry) {
        registry.register(Box::new(survey_describe::SvyMeanFactory {}));
        registry.register(Box::new(survey_describe::SvyTotalFactory {}));
        registry.register(Box::new(survey_describe::SvyVarFactory {}));
        registry.register(Box::new(survey_describe::SvyRatioFactory {}));
        registry.register(Box::new(survey_describe::SvyTableFactory {}));
        registry.register(Box::new(survey_describe::SvyQuantileFactory {}));
        registry.register(Box::new(survey_model::SvyGlmFactory {}));
        registry.register(Box::new(survey_model::SvyCoxphFactory {}));
        registry.register(Box::new(survey_model::SvySurvregFactory {}));
        registry.register(Box::new(survey_model::SvyOlrFactory {}));
        registry.register(Box::new(survey_model::SvyLoglinFactory {}));
        registry.register(Box::new(survey_model::SvyMleFactory {}));
        registry.register(Box::new(survey_model::SvyNlsFactory {}));
        registry.register(Box::new(survey_model::SvyIvregFactory {}));
        registry.register(Box::new(survey_calibrate::PostStratifyFactory {}));
        registry.register(Box::new(survey_calibrate::RakeFactory {}));
        registry.register(Box::new(survey_calibrate::CalibrateFactory {}));
        registry.register(Box::new(survey_calibrate::TrimWeightsFactory {}));
        registry.register(Box::new(survey_test::SvyTtestFactory {}));
        registry.register(Box::new(survey_test::SvyRankTestFactory {}));
        registry.register(Box::new(survey_test::SvyChisqFactory {}));
        registry.register(Box::new(survey_test::SvyCiPropFactory {}));
        registry.register(Box::new(survey_survival::SvyKmFactory {}));
        registry.register(Box::new(survey_survival::SvyLogrankFactory {}));
        registry.register(Box::new(survey_utility::SvyByFactory {}));
        registry.register(Box::new(survey_utility::SvyContrastFactory {}));
        registry.register(Box::new(survey_utility::SvyStandardizeFactory {}));
        registry.register(Box::new(survey_utility::RegTermTestFactory {}));
        registry.register(Box::new(survey_rcs::SvyRcsFactory {}));
    }
}
