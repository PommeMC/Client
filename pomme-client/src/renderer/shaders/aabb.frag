#version 450
layout(early_fragment_tests) in;
layout(location=0) flat in uint box_id;
layout(set=1,binding=2) buffer Visibility { uint visibility[]; };
void main(){atomicOr(visibility[box_id],1u);}
