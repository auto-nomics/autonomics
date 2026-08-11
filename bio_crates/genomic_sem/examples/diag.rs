//! Diagnostic: compare Rust SEM vs R lavaan golden with tight precision.
use faer::Mat;
use genomic_sem::*;

fn load_golden(name: &str) -> serde_json::Value {
    let path = format!("{}/tests/fixtures/golden/{}", env!("CARGO_MANIFEST_DIR"), name);
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn json_mat(v: &serde_json::Value) -> Mat<f64> {
    let rows = v.as_array().unwrap();
    let n = rows.len(); let m = rows[0].as_array().unwrap().len();
    let mut mat = Mat::zeros(n, m);
    for i in 0..n { let r = rows[i].as_array().unwrap(); for j in 0..m { mat[(i,j)] = r[j].as_f64().unwrap(); } }
    mat
}

fn run_scenario(name: &str) {
    let g = load_golden(name);
    let s = json_mat(&g["S"]);
    let k = s.nrows();
    let z = k*(k+1)/2;
    let v_diag: Vec<f64> = g["V_diag"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
    let v = linalg::diag_from_vec(&v_diag);
    let covstruc = utils::Covstruc { v, s, i_mat: Mat::<f64>::identity(k,k), n: Mat::zeros(1,z), m: 100000.0, v_stand: None, s_stand: None };
    let result = usermodel::usermodel(&covstruc, &usermodel::UserModelConfig {
        model: g["model"].as_str().unwrap().to_string(), ..Default::default()
    }).unwrap();

    println!("=== {} ===", g["scenario"].as_str().unwrap());

    // Loadings
    let lavaan_loadings: Vec<f64> = {
        let lhs: Vec<String> = g["par_lhs"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let op: Vec<String> = g["par_op"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let est: Vec<f64> = g["par_est"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        (0..lhs.len()).filter(|&i| op[i] == "=~").map(|i| est[i]).collect()
    };
    print!("Loadings:");
    for (i, r) in result.results.iter().filter(|r| r.op == "=~").enumerate() {
        if i < lavaan_loadings.len() {
            let rel_err = if lavaan_loadings[i].abs() > 1e-15 {
                (r.unstand_est - lavaan_loadings[i]).abs() / lavaan_loadings[i].abs() * 100.0
            } else { 0.0 };
            print!("  V{}: {:.6} vs {:.6} ({:.4}%)", i+1, r.unstand_est, lavaan_loadings[i], rel_err);
        }
    }
    println!();

    // Implied covariance
    let golden_implied = json_mat(&g["implied"]);
    let mut max_err = 0.0f64;
    for i in 0..k { for j in 0..k {
        let err = (result.sem_implied[(i,j)] - golden_implied[(i,j)]).abs();
        if err > max_err { max_err = err; }
    }}
    println!("  Max implied cov abs error: {:.2e}", max_err);
    println!();
}

fn main() {
    run_scenario("scenario1_one_factor_3trait.json");
    run_scenario("scenario2_one_factor_5trait.json");
    run_scenario("scenario3_two_factor_4trait.json");
}
