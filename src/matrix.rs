#![warn(unused_variables)]
#![warn(unused_imports)]
#![warn(dead_code)]

use anyhow::Result;
use num_complex::Complex64;
use ndarray::*;
use ndarray_linalg::*;

use crate::netlist::Netlist;
use crate::elements::{Element, ElementType};
use crate::elements::{VoltageSource, Capacitor, Resistor, Inductor};

pub enum Analysis {
    DC,
    AC,
    Tran,
}

trait ExtendWith0 {
    fn extend_with0(&mut self);
}

impl ExtendWith0 for ArrayBase<OwnedRepr<Complex64>, Dim<[usize; 2]>> {
    fn extend_with0(&mut self) {
        let shape = self.shape();
        let (m, n) = (shape[0], shape[1]);
        self.push_column(ArrayView::from(&vec![Complex64::new(0., 0.); m])).ok();
        self.push_row(ArrayView::from(&vec![Complex64::new(0., 0.); n+1])).ok();
    }
}

impl ExtendWith0 for ArrayBase<OwnedRepr<Complex64>, Dim<[usize; 1]>> {
    fn extend_with0(&mut self) {
        self.append(Axis(0), ArrayView::from(&vec![Complex64::new(0., 0.); 1]));
    }
}

fn change_vec_from_netlist(netlist: &Netlist) -> Vec<(&str, Element)> {
    let mut vec = Vec::new();
    for (inst, elem) in netlist.v.iter() {
        vec.push((inst.as_str(), *elem));
    }
    for (inst, elem) in netlist.r.iter() {
        vec.push((inst.as_str(), *elem));
    }
    for (inst, elem) in netlist.c.iter() {
        vec.push((inst.as_str(), *elem));
    }
    for (inst, elem) in netlist.l.iter() {
        vec.push((inst.as_str(), *elem));
    }
    vec
}

fn get_element_type(elem: &str) -> Option<ElementType> {
    if elem.chars().next().unwrap() == 'v' || elem.chars().next().unwrap() == 'V' {
        return Some(ElementType::V);
    } else if elem.chars().next().unwrap() == 'c' || elem.chars().next().unwrap() == 'C' {
        return Some(ElementType::C);
    } else if elem.chars().next().unwrap() == 'l' || elem.chars().next().unwrap() == 'L' {
        return Some(ElementType::L);
    } else if elem.chars().next().unwrap() == 'r' || elem.chars().next().unwrap() == 'R' {
        return Some(ElementType::R);
    } else {
        None
    }
}

fn remove_ground_from_array(matrix: &Array2<Complex64>) -> Array2<Complex64> {
    let mut new_matrix: Array2<Complex64> = Array2::from_elem((matrix.nrows() - 1, matrix.ncols() - 1), Complex64::new(0., 0.));
    for row in 1..matrix.nrows() {
        for col in 1..matrix.ncols() {
            new_matrix[[row - 1, col - 1]] = matrix[[row, col]];
        }
    }
    new_matrix
}

fn remove_ground_from_vector(vector: &Array1<Complex64>) -> Array1<Complex64> {
    let mut new_vector: Array1<Complex64> = Array1::from_elem(vector.len() - 1, Complex64::new(0., 0.));
    for index in 1..vector.len() {
        new_vector[index - 1] = vector[index];
    }
    new_vector
}

fn remove_ground_from_nodes(nodes: &Array1<usize>) -> Array1<usize> {
    let mut new_nodes: Array1<usize> = Array1::from_elem(nodes.len() - 1, 0);
    for index in 1..nodes.len() {
        new_nodes[index - 1] = nodes[index];
    }
    new_nodes
}

#[derive(Debug)]
pub struct CircuitMatrix {
    pub mat: Array2<Complex64>,
    pub vec: Array1<Complex64>,
    pub nodes: Array1<usize>, //TODO usize -> generics としたい
}

impl CircuitMatrix {
    pub fn new() -> Self {
        // 初期化された最小の２行２列のマトリックスと２要素の配列を返す
        let a = array![
            [Complex64::new(0., 0.), Complex64::new(0., 0.)], 
            [Complex64::new(0., 0.), Complex64::new(0., 0.)]
        ];
        let b = array![Complex64::new(0., 0.), Complex64::new(0., 0.)];
        let c = array![0, 1];
        CircuitMatrix { mat: a, vec: b, nodes: c }
    }

    // main method
    pub fn create_mat_vec_from_netlist(&mut self, netlist: &Netlist, analysis: Analysis, omega: f64) -> () {
        let elements = change_vec_from_netlist(&netlist);
        for element in elements.iter() {
        // element: tuple (1: instance(&str), 2: element(struct Element))
            let etype = get_element_type(element.0); //インスタンスから素子のタイプを確認
            // ノードリストをアップデートする
            self.update_nodes(element.1.pos, element.1.neg, etype.clone().unwrap());
            // element ごとのタイプを確認し、固有の行列とベクトルを格納
            let mut elem_mat_vec: (Array2<Complex64>, Array1<Complex64>);
            if let Some(etype) = etype {
                elem_mat_vec = self.gen_mat_vec_from_element(element.1, &etype);
                match analysis {
                    Analysis::DC => (),
                    Analysis::AC => { elem_mat_vec = self.ac_mat_vec(elem_mat_vec, etype, omega) },
                    Analysis::Tran => unimplemented!(),
                }
            } else {
                break;
            }
            // 素子行列とベクトルを拡張(０で補完)
            let extended_elem_mat_vec = self.extend_elem_mat_vec(elem_mat_vec, element.1.pos, element.1.neg);
            // 元の行列・ベクトルと素子の行列・ベクトルを加算
            self.add_mat_vec(&extended_elem_mat_vec);
        }
    }

    fn gen_mat_vec_from_element(&mut self, elem: Element, etype: &ElementType) -> (Array2<Complex64>, Array1<Complex64>){
        let elem_mat_vec: (Array2<Complex64>, Array1<Complex64>);
        // 素子の固有行列と固有ベクトルを取得してくる。ノードリストもアップデートする
        match etype {
            ElementType::V => elem_mat_vec = self.gen_mat_vec_V(elem),
            ElementType::C => elem_mat_vec = self.gen_mat_vec_C(elem),
            ElementType::R => elem_mat_vec = self.gen_mat_vec_R(elem),
            ElementType::L => elem_mat_vec = self.gen_mat_vec_L(elem),
        }
        elem_mat_vec
    }

    fn extend_elem_mat_vec(&mut self, elem_mat_vec: (Array2<Complex64>, Array1<Complex64>), pos: usize, neg: usize) -> (Array2<Complex64>, Array1<Complex64>) {
        // 要素０の行列とベクトルを生成
        let mut mat: Array2<Complex64> = Array::zeros((self.nodes.len(), self.nodes.len()));
        let mut vec: Array1<Complex64> = Array::zeros(self.nodes.len());
        // nodes から素子の接続されている要素インデックスを取得
        let mut node_i1: usize = 0;
        let mut node_i2: usize = 0;
        for (i, node) in self.nodes.iter().enumerate() {
            if node == &pos {
               node_i1 += i;
            } else if node == &neg{
               node_i2 += i;
            }
        }
        // 要素に加算
        if elem_mat_vec.1.len() == 2 {
        // 2行2列の場合
            let elem_mat = elem_mat_vec.0;
            mat[[node_i1, node_i1]] += elem_mat[[0, 0]];
            mat[[node_i1, node_i2]] += elem_mat[[0, 1]];
            mat[[node_i2, node_i1]] += elem_mat[[1, 0]];
            mat[[node_i2, node_i2]] += elem_mat[[1, 1]];

            let elem_vec = elem_mat_vec.1;
            vec[node_i1] += elem_vec[0];
            vec[node_i2] += elem_vec[1];
        } else if elem_mat_vec.1.len() == 3 {
        // 3行3列の場合
            let elem_mat = elem_mat_vec.0;
            let node_i3 = self.nodes.len() - 1;
            mat[[node_i1, node_i1]] += elem_mat[[0, 0]];
            mat[[node_i1, node_i2]] += elem_mat[[0, 1]];
            mat[[node_i1, node_i3]] += elem_mat[[0, 2]];
            mat[[node_i2, node_i1]] += elem_mat[[1, 0]];
            mat[[node_i2, node_i2]] += elem_mat[[1, 1]];
            mat[[node_i2, node_i3]] += elem_mat[[1, 2]];
            mat[[node_i3, node_i1]] += elem_mat[[2, 0]];
            mat[[node_i3, node_i2]] += elem_mat[[2, 1]];
            mat[[node_i3, node_i3]] += elem_mat[[2, 2]];

            let elem_vec = elem_mat_vec.1;
            vec[node_i1] += elem_vec[0];
            vec[node_i2] += elem_vec[1];
            vec[node_i3] += elem_vec[2];
        }
        (mat, vec)
    }

    fn update_nodes(&mut self, pos: usize, neg: usize, etype: ElementType) -> () {
        // self.nodesを素子のつながっているノードを見て更新する
        let is_pos = self.nodes.iter().any(|node| node==&pos);
        let is_neg = self.nodes.iter().any(|node| node==&neg);
        if !is_pos {
            self.nodes.append(
                Axis(0),
                ArrayView1::from(&[pos])
            );
            self.mat.extend_with0();
            self.vec.extend_with0();
        }
        if !is_neg {
            self.nodes.append(
                Axis(0),
                ArrayView1::from(&[neg])
            );
            self.mat.extend_with0();
            self.vec.extend_with0();
        }
        match etype {
            ElementType::V => { self.nodes.append(Axis(0), ArrayView1::from(&[100])); }, //TODO
                                                                                         //電流ノードを識別できるように修正する
            ElementType::L => { self.nodes.append(Axis(0), ArrayView1::from(&[100])); }, //TODO
            _ => (),
        }
    }

    fn ac_mat_vec(&mut self, elem_mat_vec: (Array2<Complex64>, Array1<Complex64>), etype: ElementType, omega: f64) -> (Array2<Complex64>, Array1<Complex64>) {
        let (mat, vec) = match etype {
            ElementType::V => self.ac_mat_vec_V(elem_mat_vec, omega),
            ElementType::C => self.ac_mat_vec_C(elem_mat_vec, omega),
            ElementType::R => self.ac_mat_vec_R(elem_mat_vec, omega),
            ElementType::L => self.ac_mat_vec_L(elem_mat_vec, omega),
        };
        (mat, vec)
    }

    pub fn remove_ground(&mut self) -> () {
        self.mat = remove_ground_from_array(&self.mat);
        self.vec = remove_ground_from_vector(&self.vec);
        self.nodes = remove_ground_from_nodes(&self.nodes);
    }

    fn add_mat_vec(&mut self, mat_vec: &(Array2<Complex64>, Array1<Complex64>)) -> () {
        self.mat = &self.mat + mat_vec.0.clone();
        self.vec = &self.vec + mat_vec.1.clone();
    }

    pub fn get_current_mat_vec(&self) -> (Array2<Complex64>, Array1<Complex64>) {
        (self.mat.clone(), self.vec.clone())
    }

    pub fn get_number_of_nodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn solve(&self) -> Result<Array1<Complex64>> {
        let result: Array1<Complex64> = self.mat.solve_into(self.vec.clone())?;
        Ok(result)
    }
  }

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn matrix() {
        let c1_element = Element { pos: 2, neg: 0, value: 3.0 };
        let r1_element = Element { pos: 0, neg: 1, value: 3.0 };
        let r2_element = Element { pos: 1, neg: 2, value: 3.0 };
        let mut netlist = Netlist { 
                v: HashMap::new(),
                r: HashMap::new(),
                c: HashMap::new(),
                l: HashMap::new(),
        };
        netlist.c.insert(String::from("c1"), c1_element);
        netlist.r.insert(String::from("r1"), r1_element);
        netlist.r.insert(String::from("r2"), r2_element);
    }
}
