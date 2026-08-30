#include <math.h>

#define ICSVAR_STATE 1

typedef struct {
    double capacitance;
    double dcap_dv;
    double gate_current;
    double dgate_dv;
} IcsVarEval;

static double
icsvar_logistic(double z)
{
    if (z >= 0.0) {
        double e = exp(-z);
        return e / (1.0 + e);
    } else {
        double e = exp(z);
        return 1.0 / (1.0 + e);
    }
}

static void
icsvar_eval_12(double v, double temp, double l, double w, double nf,
               double mr, double mismod, double dcgg, double dtox,
               double sigma_mis, IcsVarEval *result)
{
    const double llr = l * 0.9 * 1.1111;
    const double wwr = w * 0.9 * 1.1111;
    const double lu = llr * 1.0e6;
    const double wu = wwr * 1.0e6;
    const double dt = temp - 25.0;
    const double cgg_a2_sp =
        (1.6782 * lu + 0.79398) *
        pow(nf * wu, -0.000037202 * lu * lu + 0.0020378 * lu + 0.9748);
    const double cgg_a1_sp =
        (13.021 * lu + 0.15931) *
        pow(nf * wu, -0.000009953 * lu * lu + 0.0005767 * lu + 0.99354);
    const double cgg_x0_sp = -(-0.00006 * lu * lu + 0.0028 * lu + 0.1052) - 0.01;
    const double cgg_dx_sp =
        -((0.0000071456 * lu * lu - 0.00030298 * lu - 0.0020931) *
          (log(wu * nf) / log(2.71828)) + (0.0000016102 * lu + 0.19184));
    const double cgg_a2 =
        (-4.83074e-8 / llr + 2.65781e-8 / wwr +
         1.46441e-13 / (wwr * llr) * (1.0 - 2.62621e-4 * dt) + 0.83659) *
        cgg_a2_sp * (1.0 + 1.17801e-4 * dt);
    const double cgg_a1 =
        (2.79275e-8 / llr + 3.87915e-8 / wwr +
         3.81785e-14 / (wwr * llr) + 0.810707) *
        cgg_a1_sp * (1.0 - 7.83557e-5 * dt);
    const double cgg_x0 =
        (2.61795e-8 / llr - 3.35313e-8 / wwr +
         1.1703e-14 / (wwr * llr) + 0.721482) *
        cgg_x0_sp * (1.0 + 1.86888e-3 * dt);
    const double cgg_dx = 0.9 * cgg_dx_sp * (1.0 - 3.70895e-4 * dt);
    const double mismatch_area = w * 1.1111 * l * 1.1111 * mr * nf;
    const double geo_var = 1.416e-14 / mismatch_area +
                           5.88e-9 / sqrt(mismatch_area);
    const double dcgg_mis = mismod * sigma_mis * geo_var;
    const double temp_width = 1.0 + 1.15421e-3 * dt;
    const double z = (v - cgg_x0) / (cgg_dx * temp_width);
    const double sigmoid = icsvar_logistic(z);
    const double c_ff = cgg_a2 + (cgg_a1 - cgg_a2) * sigmoid;
    const double c_scale = (1.0 + dcgg + dcgg_mis) * 1.0e-15;
    const double unclamped_c = c_ff * c_scale;
    const double tox = 2.6e-9 + dtox;
    const double weff =
        (wwr - 1.3e-8 -
         2.0 * (-3.9e-8 + 1.0e-15 / pow(llr, 1.015) +
                8.85e-16 / wwr -
                6.4478e-23 / (pow(llr, 1.015) * wwr))) * nf;
    const double leff =
        llr - 1.2e-8 -
        2.0 * (-1.7e-8 + 1.2e-14 / pow(llr, 0.78143));
    const double gcarc = 33.0 * (1.0 + 0.0015 * dt);
    const double abs_v_gcie = pow(fabs(v), 1.5);
    const double gate_exp = exp(1.6 * v - 1000.0 * pow(tox, 0.4));
    const double gate_scale = 2.0 * mr * gcarc * weff * leff;

    if (unclamped_c > 1.0e-15) {
        result->capacitance = unclamped_c * mr;
        result->dcap_dv =
            (cgg_a1 - cgg_a2) * (-sigmoid * (1.0 - sigmoid)) /
            (cgg_dx * temp_width) * c_scale * mr;
    } else {
        result->capacitance = 1.0e-15 * mr;
        result->dcap_dv = 0.0;
    }

    result->gate_current = gate_scale * v * abs_v_gcie * gate_exp;
    result->dgate_dv = gate_scale * gate_exp *
        (2.5 * abs_v_gcie + 1.6 * v * abs_v_gcie);
}

static void
icsvar_eval_33(double v, double temp, double l, double w, double nf,
               double mr, double mismod, double dcgg, double sigma_mis,
               IcsVarEval *result)
{
    const double llr = l * 0.9 * 1.1111;
    const double wwr = w * 0.9 * 1.1111;
    const double lu = llr * 1.0e6;
    const double wu = wwr * 1.0e6;
    const double dt = temp - 25.0;
    const double cgg_a2_sp =
        (0.965 * lu + 1.1) *
        pow(nf * wu, -0.0000337539 * lu * lu + 0.00255 * lu + 0.95);
    const double cgg_a1_sp =
        (5.7539 * lu + 0.13586) * (nf * wu) + 0.0017742 * pow(lu, -6.8607);
    const double cgg_x0_sp =
        -(-0.0045 * (log(lu * wu) / log(2.71828)) + 0.3433);
    const double cgg_dx_sp = -0.3;
    const double cgg_a2 =
        (-6.73199e-8 / llr + 5.72657e-8 / wwr +
         1.1643e-13 / (wwr * llr) * (1.0 - 3.0e-3 * dt) + 0.706132) *
        cgg_a2_sp * (1.0 + 1.3047e-3 * dt);
    const double cgg_a1 =
        (5.33467e-8 / llr + 3.0213e-8 / wwr +
         7.66531e-14 / (wwr * llr) + 0.693563) *
        cgg_a1_sp * (1.0 - 1.22459e-4 * dt);
    const double cgg_x0 =
        0.927668 * cgg_x0_sp * (1.0 + 7.61167e-5 * dt);
    const double cgg_dx =
        0.98799 * cgg_dx_sp * (1.0 - 3.94865e-4 * dt);
    const double mismatch_area = w * 1.1111 * l * 1.1111 * mr * nf;
    const double geo_var = 3.78e-15 / mismatch_area +
                           2.7072e-8 / sqrt(mismatch_area);
    const double dcgg_mis = mismod * sigma_mis * geo_var;
    const double p = 1.0 - 0.06816 * v + 0.312403 * v * v +
                     0.010637 * v * v * v - 0.02437 * v * v * v * v;
    const double dp_dv = -0.06816 + 2.0 * 0.312403 * v +
                         3.0 * 0.010637 * v * v -
                         4.0 * 0.02437 * v * v * v;
    const double temp_width = 1.0 - 4.11536e-4 * dt;
    const double denom = cgg_dx * p * temp_width;
    const double ddenom_dv = cgg_dx * dp_dv * temp_width;
    const double z = (v - cgg_x0) / denom;
    const double dz_dv = 1.0 / denom -
                         (v - cgg_x0) * ddenom_dv / (denom * denom);
    const double sigmoid = icsvar_logistic(z);
    const double c_ff = cgg_a2 + (cgg_a1 - cgg_a2) * sigmoid;
    const double c_scale = (1.0 + dcgg + dcgg_mis) * 1.0e-15;
    const double unclamped_c = c_ff * c_scale;

    if (unclamped_c > 1.0e-18) {
        result->capacitance = unclamped_c * mr;
        result->dcap_dv = (cgg_a1 - cgg_a2) *
            (-sigmoid * (1.0 - sigmoid)) * dz_dv * c_scale * mr;
    } else {
        result->capacitance = 1.0e-18 * mr;
        result->dcap_dv = 0.0;
    }

    result->gate_current = 0.0;
    result->dgate_dv = 0.0;
}

void
cm_icsvar(ARGS)
{
    const double variant = PARAM(variant);
    const double voltage = INPUT(branch);
    IcsVarEval result;

    if (variant == 12.0) {
        icsvar_eval_12(voltage, TEMPERATURE, PARAM(l), PARAM(w), PARAM(nf),
                       PARAM(mr), PARAM(mismod), PARAM(dcgg), PARAM(dtox),
                       PARAM(sigma_mis), &result);
    } else if (variant == 33.0) {
        icsvar_eval_33(voltage, TEMPERATURE, PARAM(l), PARAM(w), PARAM(nf),
                       PARAM(mr), PARAM(mismod), PARAM(dcgg), PARAM(sigma_mis),
                       &result);
    } else {
        cm_message_send("icsvar: variant must be 12 or 33");
        OUTPUT(branch) = 0.0;
        PARTIAL(branch, branch) = 0.0;
        return;
    }

    if ((ANALYSIS != MIF_AC) && INIT)
        cm_analog_alloc(ICSVAR_STATE, 2 * sizeof(double));

    if (ANALYSIS == MIF_AC) {
        Mif_Complex_t admittance;
        admittance.real = result.dgate_dv;
        admittance.imag = RAD_FREQ * result.capacitance;
        AC_GAIN(branch, branch) = admittance;
    } else if (ANALYSIS == MIF_TRAN) {
        double *voltage_state;
        double dv_dt;
        double ddv_dt_dv;

        voltage_state = (double *) cm_analog_get_ptr(ICSVAR_STATE, 0);
        if (cm_analog_derivative(voltage, voltage_state, &dv_dt,
                                 &ddv_dt_dv) != MIF_OK) {
            OUTPUT(branch) = 0.0;
            PARTIAL(branch, branch) = 0.0;
            return;
        }

        OUTPUT(branch) = result.gate_current + result.capacitance * dv_dt;
        PARTIAL(branch, branch) = result.dgate_dv +
            result.dcap_dv * dv_dt + result.capacitance * ddv_dt_dv;
    } else if (ANALYSIS == MIF_DC) {
        double *voltage_state =
            (double *) cm_analog_get_ptr(ICSVAR_STATE, 0);
        double *previous_voltage_state =
            (double *) cm_analog_get_ptr(ICSVAR_STATE, 1);

        voltage_state[0] = previous_voltage_state[0] = voltage;
        voltage_state[1] = previous_voltage_state[1] = 0.0;
        OUTPUT(branch) = result.gate_current;
        PARTIAL(branch, branch) = result.dgate_dv;
    } else {
        OUTPUT(branch) = result.gate_current;
        PARTIAL(branch, branch) = result.dgate_dv;
    }
}
